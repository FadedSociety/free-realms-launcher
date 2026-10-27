//! The server-from-releases flow, against a local HTTP site that behaves like GitHub: a
//! latest-release JSON, a signed manifest asset, and file assets that 302 to a signed-looking URL.
use ed25519_dalek::{Signer, SigningKey};
use osfr_launcher::{
    patch,
    release::{self, Outcome, ReleaseSource},
};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
/// The test site's release key (the launcher pins the real one).
const SEED: [u8; 32] = [7; 32];
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .join("osfr-launcher-test-work")
            .join(format!(
                "release-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn server(&self) -> PathBuf {
        self.0.join("server")
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

type Routes = HashMap<String, (u16, Option<String>, Vec<u8>)>;
struct Site {
    base: String,
    routes: Arc<Mutex<Routes>>,
    hits: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Site {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: Arc<Mutex<Routes>> = Arc::default();
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (served, seen, stopping) = (routes.clone(), hits.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                // An accepted socket inherits the listener's non-blocking mode on Windows.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap_or("").to_owned();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                seen.lock().unwrap().push(path.clone());
                let (code, location, body) = served
                    .lock()
                    .unwrap()
                    .get(path.split('?').next().unwrap())
                    .cloned()
                    .unwrap_or((404, None, Vec::new()));
                let location = location
                    .map(|l| format!("Location: {l}\r\n"))
                    .unwrap_or_default();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {code} X\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        Self {
            base,
            routes,
            hits,
            stop,
            worker: Some(worker),
        }
    }
    fn route(&self, path: &str, code: u16, location: Option<String>, body: Vec<u8>) {
        self.routes
            .lock()
            .unwrap()
            .insert(path.to_owned(), (code, location, body));
    }
    /// Publish `tag` as the latest release. Each file is an asset named after its path, served
    /// through a redirect like GitHub's. `corrupt` flips the first byte of that path's download.
    fn publish(&self, tag: &str, files: &[(&str, &[u8])], corrupt: Option<&str>) {
        self.route(
            "/repos/x/releases/latest",
            200,
            None,
            format!(r#"{{"tag_name":"{tag}","name":"ignored","assets":[]}}"#).into_bytes(),
        );
        let mut entries = Vec::new();
        for (path, bytes) in files {
            let asset = path.replace('/', "_");
            entries.push(serde_json::json!({
                "path": path, "size": bytes.len(),
                "sha256": patch::sha256(bytes).unwrap(), "asset": asset,
            }));
            let mut served = bytes.to_vec();
            if corrupt == Some(*path) {
                served[0] ^= 1;
            }
            self.route(
                &format!("/download/{tag}/{asset}"),
                302,
                Some(format!(
                    "{}/blob/{tag}/{asset}?sig=a1b2%3D&exp=9",
                    self.base
                )),
                Vec::new(),
            );
            self.route(&format!("/blob/{tag}/{asset}"), 200, None, served);
        }
        let manifest = serde_json::json!({"version": 1, "tag": tag, "files": entries});
        self.manifest(tag, serde_json::to_vec(&manifest).unwrap());
    }
    /// Serve `bytes` as `tag`'s manifest with a signature from the test key.
    fn manifest(&self, tag: &str, bytes: Vec<u8>) {
        let signature = SigningKey::from_bytes(&SEED).sign(&bytes).to_bytes();
        self.route(
            &format!("/download/{tag}/server-manifest.json.sig"),
            200,
            None,
            signature.to_vec(),
        );
        self.route(
            &format!("/download/{tag}/server-manifest.json"),
            200,
            None,
            bytes,
        );
    }
    /// Publish a manifest of arbitrary JSON entries (unsafe paths, huge sizes).
    fn publish_raw(&self, tag: &str, entries: serde_json::Value) {
        self.route(
            "/repos/x/releases/latest",
            200,
            None,
            format!(r#"{{"tag_name":"{tag}"}}"#).into_bytes(),
        );
        self.manifest(
            tag,
            serde_json::to_vec(&serde_json::json!({"version": 1, "tag": tag, "files": entries}))
                .unwrap(),
        );
    }
    fn source(&self) -> ReleaseSource {
        ReleaseSource {
            api_url: format!("{}/repos/x/releases/latest", self.base),
            download_base: format!("{}/download", self.base),
            key: SigningKey::from_bytes(&SEED).verifying_key().to_bytes(),
        }
    }
    fn sync(&self, server: &Path) -> Result<Outcome, String> {
        release::sync(
            server,
            &self.source(),
            false,
            &AtomicBool::new(false),
            |_| {},
        )
    }
    fn blob_hits(&self) -> usize {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with("/blob/"))
            .count()
    }
    fn clear_hits(&self) {
        self.hits.lock().unwrap().clear();
    }
}
impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

/// Every file under `dir` with its bytes, relative and sorted: a whole-tree snapshot.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let name = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((name, fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    if dir.is_dir() {
        walk(dir, dir, &mut out);
    }
    out.sort();
    out
}

const V1: [(&str, &[u8]); 3] = [
    ("bin/osfr-gateway.exe", b"gateway v1"),
    ("content/quests.json", b"quests v1"),
    ("keys/client-login-key.bin", b"login key v1"),
];

#[test]
fn first_install_downloads_verifies_and_marks_the_release() {
    let (folder, site) = (Folder::new(), Site::new());
    site.publish("v1", &V1, None);
    let outcome = site.sync(&folder.server()).unwrap();
    assert!(outcome.installed);
    assert_eq!((outcome.tag.as_str(), outcome.files_downloaded), ("v1", 3));
    for (path, bytes) in V1 {
        assert_eq!(fs::read(folder.server().join(path)).unwrap(), bytes);
    }
    // The staging, journal and moved-aside folders are gone; only the release marker remains.
    let hidden: Vec<_> = fs::read_dir(folder.server())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert_eq!(hidden, [".osfr-release.json"]);
}

#[test]
fn up_to_date_is_a_no_op_that_downloads_nothing() {
    let (folder, site) = (Folder::new(), Site::new());
    site.publish("v1", &V1, None);
    site.sync(&folder.server()).unwrap();
    let before = snapshot(&folder.server());
    site.clear_hits();
    let outcome = site.sync(&folder.server()).unwrap();
    assert!(!outcome.installed && outcome.files_downloaded == 0);
    assert_eq!(*site.hits.lock().unwrap(), ["/repos/x/releases/latest"]);
    assert_eq!(snapshot(&folder.server()), before);
}

#[test]
fn an_update_downloads_only_the_changed_file_and_drops_removed_ones() {
    let (folder, site) = (Folder::new(), Site::new());
    site.publish("v1", &V1, None);
    site.sync(&folder.server()).unwrap();
    fs::write(folder.server().join("bin/mine.txt"), b"my own file").unwrap();
    site.publish(
        "v2",
        &[
            ("bin/osfr-gateway.exe", b"gateway v2"),
            ("keys/client-login-key.bin", b"login key v1"),
        ],
        None,
    );
    site.clear_hits();
    let outcome = site.sync(&folder.server()).unwrap();
    assert_eq!((outcome.tag.as_str(), outcome.files_downloaded), ("v2", 1));
    assert_eq!(site.blob_hits(), 1);
    let server = folder.server();
    assert_eq!(
        fs::read(server.join("bin/osfr-gateway.exe")).unwrap(),
        b"gateway v2"
    );
    assert!(
        !server.join("content/quests.json").exists(),
        "dropped by v2"
    );
    assert_eq!(
        fs::read(server.join("bin/mine.txt")).unwrap(),
        b"my own file"
    );
    assert!(
        fs::read_to_string(server.join(".osfr-release.json"))
            .unwrap()
            .contains("v2")
    );
}

#[test]
fn a_corrupted_download_is_refused_and_the_old_server_stays_whole() {
    let (folder, site) = (Folder::new(), Site::new());
    site.publish("v1", &V1, None);
    site.sync(&folder.server()).unwrap();
    let before = snapshot(&folder.server());
    // Two files change; the second arrives corrupted. The first, though verified, must not land.
    site.publish(
        "v2",
        &[
            ("bin/osfr-gateway.exe", b"gateway v2"),
            ("content/quests.json", b"quests v2"),
            ("keys/client-login-key.bin", b"login key v1"),
        ],
        Some("content/quests.json"),
    );
    let error = site.sync(&folder.server()).unwrap_err();
    assert!(error.contains("SHA-256"), "{error}");
    let after = snapshot(&folder.server());
    let live = |files: &[(String, Vec<u8>)]| {
        files
            .iter()
            .filter(|(n, _)| !n.starts_with('.'))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        live(&after),
        live(&before),
        "the previous server is untouched"
    );
    assert!(
        fs::read_to_string(folder.server().join(".osfr-release.json"))
            .unwrap()
            .contains("v1"),
        "the marker still names the installed release"
    );
}

#[test]
fn unsafe_manifests_are_refused_before_any_download_or_write() {
    for path in [
        "../escape.bin",
        "/absolute.bin",
        "C:/absolute.bin",
        "//server/share/x.bin",
        "a/../escape.bin",
        "a\\b.bin",
        "a:stream",
        "NUL.bin",
        "COM1",
        "a. ",
        ".update-stage/x",
        "state/character.json",
        "STATE/character.json",
        "state",
    ] {
        let (folder, site) = (Folder::new(), Site::new());
        site.publish_raw(
            "v9",
            serde_json::json!([{"path": path, "size": 3, "sha256": "00".repeat(32), "asset": "x"}]),
        );
        assert!(site.sync(&folder.server()).is_err(), "accepted {path}");
        assert_eq!(site.blob_hits(), 0, "{path}");
        assert!(snapshot(&folder.0).is_empty(), "wrote something for {path}");
    }
}

#[test]
fn bad_assets_tags_and_oversized_files_are_refused() {
    let (folder, site) = (Folder::new(), Site::new());
    for (tag, asset, size) in [
        ("v9", "../x", 3_u64),
        ("v9", "a/b", 3),
        ("v9", "", 3),
        ("v9", "ok", 3 << 30), // over the 2 GiB per-file cap
        ("../v9", "ok", 3),
    ] {
        site.publish_raw(
            tag,
            serde_json::json!([{"path": "a.bin", "size": size, "sha256": "00".repeat(32), "asset": asset}]),
        );
        assert!(site.sync(&folder.server()).is_err(), "{tag} {asset} {size}");
        assert!(!folder.server().join("a.bin").exists());
    }
    // A manifest for another tag than the release being installed is refused too.
    site.publish("v1", &V1, None);
    let other = site.routes.lock().unwrap()["/download/v1/server-manifest.json"].clone();
    let text = String::from_utf8(other.2)
        .unwrap()
        .replace(r#""tag":"v1""#, r#""tag":"v0""#);
    site.manifest("v1", text.into_bytes());
    let error = site.sync(&folder.server()).unwrap_err();
    assert!(error.contains("different release"), "{error}");
}

#[test]
fn a_manifest_without_a_valid_release_signature_installs_nothing() {
    let (folder, site) = (Folder::new(), Site::new());
    let sig = "/download/v1/server-manifest.json.sig";
    let attempts: [(&str, Box<dyn Fn(&Site)>); 4] = [
        ("missing", Box::new(|s| s.route(sig, 404, None, Vec::new()))),
        ("short", Box::new(|s| s.route(sig, 200, None, vec![0; 63]))),
        (
            "other key",
            Box::new(|s| {
                let bytes = s.routes.lock().unwrap()["/download/v1/server-manifest.json"]
                    .2
                    .clone();
                let forged = SigningKey::from_bytes(&[8; 32]).sign(&bytes).to_bytes();
                s.route(sig, 200, None, forged.to_vec());
            }),
        ),
        (
            "edited after signing",
            Box::new(|s| {
                let mut bytes = s.routes.lock().unwrap()["/download/v1/server-manifest.json"]
                    .2
                    .clone();
                bytes.push(b' ');
                s.route("/download/v1/server-manifest.json", 200, None, bytes);
            }),
        ),
    ];
    for (name, tamper) in attempts {
        site.publish("v1", &V1, None);
        tamper(&site);
        assert!(site.sync(&folder.server()).is_err(), "{name}");
        assert!(
            !folder.server().join("bin/osfr-gateway.exe").exists(),
            "{name}"
        );
    }
    // The real release key is pinned by default.
    assert_eq!(ReleaseSource::default().key, release::RELEASE_KEY);
}

#[test]
fn player_data_and_key_files_survive_every_update() {
    let (folder, site) = (Folder::new(), Site::new());
    let server = folder.server();
    fs::create_dir_all(server.join("state")).unwrap();
    fs::write(server.join("state/character.json"), b"my character").unwrap();
    fs::write(server.join("state/identity.json"), b"my identity").unwrap();
    site.publish("v1", &V1, None);
    site.sync(&server).unwrap();
    // v2 changes the key file, v3 drops it: neither may touch it. v2 changes a normal file.
    site.publish(
        "v2",
        &[
            ("bin/osfr-gateway.exe", b"gateway v2"),
            ("keys/client-login-key.bin", b"login key v2"),
        ],
        None,
    );
    site.sync(&server).unwrap();
    assert_eq!(
        fs::read(server.join("keys/client-login-key.bin")).unwrap(),
        b"login key v1"
    );
    site.publish("v3", &[("bin/osfr-gateway.exe", b"gateway v3")], None);
    site.sync(&server).unwrap();
    assert_eq!(
        fs::read(server.join("keys/client-login-key.bin")).unwrap(),
        b"login key v1"
    );
    assert_eq!(
        fs::read(server.join("bin/osfr-gateway.exe")).unwrap(),
        b"gateway v3"
    );
    assert_eq!(
        fs::read(server.join("state/character.json")).unwrap(),
        b"my character"
    );
    assert_eq!(
        fs::read(server.join("state/identity.json")).unwrap(),
        b"my identity"
    );
}

#[test]
fn plain_http_off_loopback_and_off_site_redirects_are_refused() {
    let (folder, site) = (Folder::new(), Site::new());
    let run = |source: ReleaseSource| {
        release::sync(
            &folder.server(),
            &source,
            false,
            &AtomicBool::new(false),
            |_| {},
        )
    };
    for (api, base) in [
        (
            "http://example.com/repos/x/releases/latest",
            "http://example.com/d",
        ),
        (
            "https://example.com/repos/x/releases/latest",
            "https://example.com/d",
        ),
        (
            "http://192.0.2.1/repos/x/releases/latest",
            "http://192.0.2.1/d",
        ),
        (
            "https://api.github.com/repos/x/releases/latest",
            "http://example.com/d",
        ),
        ("https://github.com@evil.example/x", "https://github.com/d"),
    ] {
        let error = run(ReleaseSource {
            api_url: api.into(),
            download_base: base.into(),
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.contains("HTTP"), "{api}: {error}");
    }
    // A download that redirects off GitHub (or to plain http elsewhere) is refused.
    for target in [
        "https://example.com/blob",
        "http://example.com/blob",
        "//example.com/blob",
    ] {
        site.publish("v1", &V1[..1], None);
        site.route(
            "/download/v1/bin_osfr-gateway.exe",
            302,
            Some(target.into()),
            Vec::new(),
        );
        let error = site.sync(&folder.server()).unwrap_err();
        assert!(error.contains("redirect"), "{target}: {error}");
        assert!(!folder.server().join("bin/osfr-gateway.exe").exists());
    }
}

#[test]
fn offline_keeps_the_installed_server_and_reports_it() {
    let (folder, site) = (Folder::new(), Site::new());
    site.publish("v1", &V1, None);
    site.sync(&folder.server()).unwrap();
    let before = snapshot(&folder.server());
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let source = ReleaseSource {
        api_url: format!("http://{closed}/repos/x/releases/latest"),
        download_base: format!("http://{closed}/download"),
        ..Default::default()
    };
    let result = release::sync(
        &folder.server(),
        &source,
        false,
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(result.is_err());
    assert_eq!(snapshot(&folder.server()), before);
}

#[test]
fn a_crash_mid_swap_is_rolled_back_on_the_next_run() {
    let (folder, site) = (Folder::new(), Site::new());
    let server = folder.server();
    site.publish("v1", &V1, None);
    site.sync(&server).unwrap();
    let before = snapshot(&server);
    // Fake the debris of a crash after the gateway was replaced but before the marker moved:
    // the journal names v2, the old bytes sit aside, and the new bytes are live.
    fs::write(
        server.join(".update-journal"),
        "v2\nM bin/osfr-gateway.exe\nA bin/new.dll\n",
    )
    .unwrap();
    fs::create_dir_all(server.join(".update-old/bin")).unwrap();
    fs::write(
        server.join(".update-old/bin/osfr-gateway.exe"),
        b"gateway v1",
    )
    .unwrap();
    fs::write(server.join("bin/osfr-gateway.exe"), b"half-installed v2").unwrap();
    fs::write(server.join("bin/new.dll"), b"half-installed").unwrap();
    site.sync(&server).unwrap(); // latest is still v1: rollback, then up to date
    assert_eq!(snapshot(&server), before);
}
