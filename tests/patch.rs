//! Synthetic bytes only, except the ignored client-overlay check; local HTTP
//! peers exercise the real WinHTTP adapter.
use osfr_launcher::patch::{self, PatchSource};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .join("osfr-launcher-test-work")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[derive(Default)]
struct PeerState {
    requests: Vec<(String, u64)>,
    kill: usize,
    lie: bool,
    bad_range: bool,
    ignore_range: bool,
    corrupt: bool,
    slow_headers: bool,
}
struct Peer {
    url: String,
    pin: String,
    state: Arc<Mutex<PeerState>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
fn digest(bytes: &[u8]) -> String {
    patch::sha256(bytes).unwrap()
}
fn manifest(files: &[(&str, &[u8])]) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"version":1, "files":files.iter().map(|(path, bytes)| {
        serde_json::json!({"path":path,"size":bytes.len(),"sha256":digest(bytes)})
    }).collect::<Vec<_>>()}),
    )
    .unwrap()
}
impl Peer {
    fn new(files: Vec<(&str, Vec<u8>)>, raw: Option<Vec<u8>>, state: PeerState) -> Self {
        let raw = raw.unwrap_or_else(|| {
            manifest(
                &files
                    .iter()
                    .map(|(p, b)| (*p, b.as_slice()))
                    .collect::<Vec<_>>(),
            )
        });
        let pin = digest(&raw);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/manifest.json", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(state));
        let shared = state.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let files: Vec<_> = files
            .into_iter()
            .map(|(p, b)| (format!("/{p}"), b))
            .collect();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut first = String::new();
                        if reader.read_line(&mut first).is_err() {
                            continue;
                        }
                        let path = first.split_whitespace().nth(1).unwrap_or("").to_owned();
                        let mut offset = 0;
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                                break;
                            }
                            if let Some(value) =
                                line.to_ascii_lowercase().strip_prefix("range: bytes=")
                            {
                                offset = value.trim().trim_end_matches('-').parse().unwrap();
                            }
                        }
                        let mut state = shared.lock().unwrap();
                        state.requests.push((path.clone(), offset));
                        let bytes = if path == "/manifest.json" {
                            &raw
                        } else {
                            match files.iter().find(|(p, _)| p == &path) {
                                Some((_, b)) => b,
                                None => {
                                    let _ = stream.write_all(b"HTTP/1.1 404 Missing\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                    continue;
                                }
                            }
                        };
                        let is_file = path != "/manifest.json";
                        let start = if state.ignore_range {
                            0
                        } else {
                            offset as usize
                        };
                        let body = &bytes[start.min(bytes.len())..];
                        let size = body.len() + usize::from(is_file && state.lie);
                        let range = if start > 0 {
                            format!(
                                "Content-Range: bytes {}-{}/{}\r\n",
                                start + usize::from(state.bad_range),
                                bytes.len() - 1,
                                bytes.len()
                            )
                        } else {
                            String::new()
                        };
                        let code = if start > 0 {
                            "206 Partial Content"
                        } else {
                            "200 OK"
                        };
                        let headers = format!(
                            "HTTP/1.1 {code}\r\nContent-Length: {size}\r\n{range}Connection: close\r\n\r\n"
                        );
                        let slow_headers = is_file && state.slow_headers;
                        let kill = is_file && state.kill > 0;
                        if kill {
                            state.kill -= 1;
                        }
                        let mut body = if kill {
                            body[..body.len() / 2].to_vec()
                        } else {
                            body.to_vec()
                        };
                        if is_file && state.corrupt && !body.is_empty() {
                            body[0] ^= 1;
                        }
                        drop(state);
                        if slow_headers {
                            stream.set_nodelay(true).unwrap();
                            for byte in headers.bytes() {
                                if stopping.load(Ordering::Relaxed)
                                    || stream.write_all(&[byte]).is_err()
                                {
                                    break;
                                }
                                thread::sleep(Duration::from_millis(100));
                            }
                        } else {
                            let _ = stream.write_all(headers.as_bytes());
                        }
                        let _ = stream.write_all(&body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            url,
            pin,
            state,
            stop,
            worker: Some(worker),
        }
    }
    fn source(&self) -> PatchSource {
        PatchSource {
            manifest_url: self.url.clone(),
            manifest_sha256: self.pin.clone(),
            mirror_url: None,
        }
    }
    fn run(&self, folder: &Folder) -> Result<patch::PatchReport, String> {
        patch::repair(&folder.0, &self.source(), &AtomicBool::new(false), |_| {})
    }
    fn downloads(&self) -> Vec<u64> {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(p, _)| p != "/manifest.json")
            .map(|(_, o)| *o)
            .collect()
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn sha256_known_vectors() {
    assert_eq!(
        digest(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
#[test]
fn fresh_install_and_only_one_changed_file() {
    let folder = Folder::new();
    let peer = Peer::new(
        vec![
            ("data/a.bin", b"first".to_vec()),
            ("b.bin", b"second".to_vec()),
        ],
        None,
        PeerState::default(),
    );
    assert_eq!(peer.run(&folder).unwrap().files_downloaded, 2);
    assert_eq!(fs::read(folder.0.join("data/a.bin")).unwrap(), b"first");
    assert_eq!(peer.run(&folder).unwrap().files_downloaded, 0);
    fs::write(folder.0.join("b.bin"), b"broken").unwrap();
    assert_eq!(peer.run(&folder).unwrap().files_downloaded, 1);
    assert_eq!(peer.downloads(), vec![0, 0, 0]);
}
#[test]
fn killed_download_resumes_across_runs() {
    let folder = Folder::new();
    let bytes = vec![42; 256 * 1024];
    let peer = Peer::new(
        vec![("a.bin", bytes.clone())],
        None,
        PeerState {
            kill: 100,
            ..Default::default()
        },
    );
    assert!(peer.run(&folder).is_err());
    let partial = fs::metadata(folder.0.join("a.bin.osfr-part"))
        .unwrap()
        .len();
    assert!(partial > 0 && partial < bytes.len() as u64);
    assert!(!folder.0.join("a.bin").exists());
    peer.state.lock().unwrap().kill = 0;
    peer.run(&folder).unwrap();
    assert!(peer.downloads().contains(&partial));
    assert_eq!(fs::read(folder.0.join("a.bin")).unwrap(), bytes);
}
#[test]
fn process_killed_mid_download_resumes() {
    if let Ok(client) = std::env::var("W121_PATCH_CHILD") {
        let source = PatchSource {
            manifest_url: std::env::var("W121_PATCH_URL").unwrap(),
            manifest_sha256: std::env::var("W121_PATCH_PIN").unwrap(),
            mirror_url: None,
        };
        patch::repair(
            std::path::Path::new(&client),
            &source,
            &AtomicBool::new(false),
            |p| {
                if p.phase == "Downloading" && p.bytes_done > 0 {
                    std::process::exit(73);
                }
            },
        )
        .unwrap();
        panic!("child never reached mid-download exit");
    }
    let folder = Folder::new();
    let bytes = vec![53; 256 * 1024];
    let peer = Peer::new(vec![("a.bin", bytes.clone())], None, PeerState::default());
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "process_killed_mid_download_resumes",
            "--nocapture",
        ])
        .env("W121_PATCH_CHILD", &folder.0)
        .env("W121_PATCH_URL", &peer.url)
        .env("W121_PATCH_PIN", &peer.pin)
        .output()
        .unwrap();
    assert_eq!(child.status.code(), Some(73));
    let partial = fs::metadata(folder.0.join("a.bin.osfr-part"))
        .unwrap()
        .len();
    assert!(partial > 0 && partial < bytes.len() as u64);
    assert!(!folder.0.join("a.bin").exists());
    peer.run(&folder).unwrap();
    assert_eq!(peer.downloads(), vec![0, partial]);
    assert_eq!(fs::read(folder.0.join("a.bin")).unwrap(), bytes);
}
#[test]
fn corrupt_partial_is_rejected_before_replace_and_restarted() {
    let folder = Folder::new();
    let peer = Peer::new(
        vec![("a.bin", b"correct content".to_vec())],
        None,
        PeerState::default(),
    );
    fs::write(folder.0.join("a.bin.osfr-part"), b"WRONG").unwrap();
    let mut installed_bad = false;
    patch::repair(&folder.0, &peer.source(), &AtomicBool::new(false), |_| {
        if folder.0.join("a.bin").exists() {
            installed_bad |= fs::read(folder.0.join("a.bin")).unwrap() != b"correct content";
        }
    })
    .unwrap();
    assert!(!installed_bad);
    assert_eq!(peer.downloads(), vec![5, 0]);
    assert_eq!(
        fs::read(folder.0.join("a.bin")).unwrap(),
        b"correct content"
    );
}
#[test]
fn lying_size_or_hash_never_replaces_existing_file() {
    for corrupt in [false, true] {
        let folder = Folder::new();
        fs::write(folder.0.join("a.bin"), b"old").unwrap();
        let peer = Peer::new(
            vec![("a.bin", b"new bytes".to_vec())],
            None,
            PeerState {
                lie: !corrupt,
                corrupt,
                ..Default::default()
            },
        );
        assert!(peer.run(&folder).is_err());
        assert_eq!(fs::read(folder.0.join("a.bin")).unwrap(), b"old");
    }
}
#[test]
fn path_traversal_and_windows_aliases_are_rejected_before_writes() {
    for path in [
        "../escape.bin",
        "/absolute.bin",
        "C:/absolute.bin",
        "a/../escape.bin",
        "a\\b",
        "a:stream",
        "NUL.bin",
        "a. ",
        "a.bin.osfr-part",
        ".friend-launcher-backups/a",
    ] {
        let folder = Folder::new();
        let raw = manifest(&[(path, b"unsafe")]);
        let peer = Peer::new(vec![], Some(raw), PeerState::default());
        assert!(peer.run(&folder).is_err(), "accepted {path}");
        assert_eq!(fs::read_dir(&folder.0).unwrap().count(), 0);
        assert!(peer.downloads().is_empty());
    }
}
#[test]
fn verify_and_repair_rehashes_same_size_tampering_canary() {
    let folder = Folder::new();
    let peer = Peer::new(
        vec![("a.bin", b"original".to_vec())],
        None,
        PeerState::default(),
    );
    peer.run(&folder).unwrap();
    fs::write(folder.0.join("a.bin"), b"tampered").unwrap();
    assert_eq!(peer.run(&folder).unwrap().files_downloaded, 1);
    assert_eq!(fs::read(folder.0.join("a.bin")).unwrap(), b"original");
}
#[test]
fn cancellation_keeps_partial_and_blocks_install() {
    let folder = Folder::new();
    let peer = Peer::new(
        vec![("a.bin", vec![7; 256 * 1024])],
        None,
        PeerState::default(),
    );
    let cancel = AtomicBool::new(false);
    assert!(
        patch::repair(&folder.0, &peer.source(), &cancel, |p| {
            if p.bytes_done > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .is_err()
    );
    assert!(!folder.0.join("a.bin").exists());
    assert!(folder.0.join("a.bin.osfr-part").exists());
}

#[test]
fn trickled_headers_cannot_hold_cancellation_open() {
    let folder = Folder::new();
    let peer = Peer::new(
        vec![("a.bin", b"fixture".to_vec())],
        None,
        PeerState {
            slow_headers: true,
            ..Default::default()
        },
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let stopping = cancel.clone();
    let state = peer.state.clone();
    let timer = thread::spawn(move || {
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < until {
            if state
                .lock()
                .unwrap()
                .requests
                .iter()
                .any(|(p, _)| p == "/a.bin")
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(100));
        stopping.store(true, Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let result = patch::repair(&folder.0, &peer.source(), &cancel, |_| {});
    timer.join().unwrap();
    assert!(result.is_err());
    assert!(!peer.downloads().is_empty());
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(!folder.0.join("a.bin").exists());
}
#[test]
fn pin_mismatch_and_remote_plain_http_are_rejected() {
    let folder = Folder::new();
    let peer = Peer::new(vec![], None, PeerState::default());
    let mut source = peer.source();
    source.manifest_sha256 = "00".repeat(32);
    assert!(patch::repair(&folder.0, &source, &AtomicBool::new(false), |_| {}).is_err());
    source.manifest_url = "http://example.com/manifest.json".into();
    assert!(patch::repair(&folder.0, &source, &AtomicBool::new(false), |_| {}).is_err());
}
#[test]
fn ignored_range_restarts_and_bad_range_fails_closed() {
    for bad_range in [false, true] {
        let folder = Folder::new();
        let peer = Peer::new(
            vec![("a.bin", b"correct".to_vec())],
            None,
            PeerState {
                bad_range,
                ignore_range: !bad_range,
                ..Default::default()
            },
        );
        fs::write(folder.0.join("a.bin.osfr-part"), b"cor").unwrap();
        let result = peer.run(&folder);
        if bad_range {
            assert!(result.is_err());
            assert!(!folder.0.join("a.bin").exists());
        } else {
            result.unwrap();
            assert_eq!(fs::read(folder.0.join("a.bin")).unwrap(), b"correct");
        }
    }
}
#[test]
fn mirror_repairs_after_primary_fails() {
    let folder = Folder::new();
    let primary = Peer::new(
        vec![("a.bin", b"correct".to_vec())],
        None,
        PeerState {
            lie: true,
            ..Default::default()
        },
    );
    let mirror = Peer::new(
        vec![("a.bin", b"correct".to_vec())],
        None,
        PeerState::default(),
    );
    let mut source = primary.source();
    source.mirror_url = Some(mirror.url.clone());
    patch::repair(&folder.0, &source, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(!primary.downloads().is_empty());
    assert_eq!(mirror.downloads(), vec![0]);
}
#[test]
fn hard_links_and_junctions_are_rejected() {
    let folder = Folder::new();
    let outside = Folder::new();
    fs::write(outside.0.join("canary"), b"outside").unwrap();
    fs::hard_link(outside.0.join("canary"), folder.0.join("a.bin")).unwrap();
    let peer = Peer::new(
        vec![("a.bin", b"patched".to_vec())],
        None,
        PeerState::default(),
    );
    assert!(peer.run(&folder).is_err());
    assert_eq!(fs::read(outside.0.join("canary")).unwrap(), b"outside");
    fs::remove_file(folder.0.join("a.bin")).unwrap();
    #[cfg(windows)]
    {
        // cmd's mklink is used only to create this test junction, never to move/delete paths.
        let result = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(folder.0.join("data"))
            .arg(&outside.0)
            .output()
            .unwrap();
        assert!(result.status.success());
        let peer = Peer::new(
            vec![("data/a.bin", b"patched".to_vec())],
            None,
            PeerState::default(),
        );
        assert!(peer.run(&folder).is_err());
        fs::remove_dir(folder.0.join("data")).unwrap();
        assert!(!outside.0.join("a.bin").exists());
    }
}
#[test]
fn partial_hard_link_and_manifest_aliases_fail_closed() {
    let folder = Folder::new();
    let outside = Folder::new();
    fs::write(outside.0.join("canary"), b"outside").unwrap();
    fs::hard_link(outside.0.join("canary"), folder.0.join("a.bin.osfr-part")).unwrap();
    let peer = Peer::new(
        vec![("a.bin", b"patched".to_vec())],
        None,
        PeerState::default(),
    );
    assert!(peer.run(&folder).is_err());
    assert_eq!(fs::read(outside.0.join("canary")).unwrap(), b"outside");
    for names in [["a.bin", "A.bin"], ["data", "data/a.bin"]] {
        let folder = Folder::new();
        let peer = Peer::new(
            vec![],
            Some(manifest(&[(names[0], b"one"), (names[1], b"two")])),
            PeerState::default(),
        );
        assert!(peer.run(&folder).is_err());
        assert_eq!(fs::read_dir(&folder.0).unwrap().count(), 0);
    }
}
/// L024 evidence: build the real overlay with `python -I scripts/bundle/client_overlay.py
/// --out <dir>`, then run with OSFR_CLIENT_OVERLAY=<dir> and `-- --ignored`. W121 installs
/// every overlay file into a fresh client copy byte for byte, replacing the client's own
/// table, and a second check downloads nothing.
#[test]
#[ignore]
fn client_overlay_lands_byte_identical() {
    let overlay = PathBuf::from(
        std::env::var("OSFR_CLIENT_OVERLAY").expect("OSFR_CLIENT_OVERLAY names a built overlay"),
    );
    let raw = fs::read(overlay.join("manifest.json")).unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let paths: Vec<&str> = listed["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    let peer = Peer::new(
        paths
            .iter()
            .map(|path| (*path, fs::read(overlay.join(path)).unwrap()))
            .collect(),
        Some(raw),
        PeerState::default(),
    );
    let client = std::env::temp_dir().join(format!("osfr-client-overlay-{}", std::process::id()));
    fs::create_dir_all(client.join("Resources")).unwrap();
    fs::write(
        client.join("Resources/CharacterCreateCustomizationSkinTone.txt"),
        b"the client's own table",
    )
    .unwrap();
    let check = || patch::repair(&client, &peer.source(), &AtomicBool::new(false), |_| {});
    assert_eq!(check().unwrap().files_downloaded, paths.len());
    for path in &paths {
        assert_eq!(
            fs::read(client.join(path)).unwrap(),
            fs::read(overlay.join(path)).unwrap(),
            "{path}"
        );
    }
    assert_eq!(check().unwrap().files_downloaded, 0);
    fs::remove_dir_all(&client).unwrap();
}
