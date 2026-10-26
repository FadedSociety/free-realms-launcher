use super::{
    LaunchPlan, LaunchSettings, LocalLoginStub, LoginProvider, SessionToken, validate_endpoint,
};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpStream,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".test-data")
            .join(format!(
                "{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.join("locale")).unwrap();
        fs::write(
            root.join("FreeRealms.exe"),
            b"MZ synthetic fixture; never execute",
        )
        .unwrap();
        fs::write(root.join("FreeRealms.bat"), "start /min FreeRealms.exe inifile=ClientConfig.ini Guid=9 Server=old:20042 Ticket=discard Key=discard CasSessionId=discard Internationalization:Locale=8 Country=US\n").unwrap();
        fs::write(root.join("ClientConfig.ini"), "Server=old:20042\nIndirectServerAddress=http://invalid.example/assets\nAddress=lvsadp-old\n").unwrap();
        fs::write(root.join("UserOptions.ini"), "WindowedWidth=1920\nWindowedHeight=1080\nMode=Fullscreen\nMaximized=1\nMaster=1.000000\n").unwrap();
        fs::write(root.join("Assets_manifest.txt"), b"123456789").unwrap(); // CRC-32 check input.
        for name in ["en_us_data.dat", "en_us_data.dir"] {
            fs::write(root.join("locale").join(name), b"old fixture").unwrap();
        }
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        let target = self.0.canonicalize().unwrap();
        assert!(target.starts_with(root.join(".test-data")));
        fs::remove_dir_all(target).unwrap();
    }
}

#[test]
fn endpoint_and_login_boundaries() {
    for value in ["127.0.0.1:20042", "realm.example:20042", "[::1]:20042"] {
        assert!(validate_endpoint(value).is_ok(), "{value}");
    }
    for value in [
        "host:0",
        "host:65536",
        "host:1;whoami",
        "http://host:20042",
        "::1:20042",
        "a..b:1",
        "[host]:1",
        "host:１２",
        "-x:10",
    ] {
        assert!(validate_endpoint(value).is_err(), "{value}");
    }
    let mut login = LocalLoginStub;
    assert!(login.session("127.0.0.1:20042").is_ok());
    assert!(login.session("[::1]:20042").is_ok());
    assert!(login.session("192.0.2.1:20042").is_err());
    assert!(login.session("localhost:20042").is_err()); // No DNS trust in the stub.
}

#[test]
fn preparation_preserves_inputs_backs_up_and_filters_auth() {
    let scratch = Scratch::new();
    let overlay = scratch.0.join("overlay");
    fs::create_dir(&overlay).unwrap();
    for name in ["en_us_data.dat", "en_us_data.dir"] {
        fs::write(overlay.join(name), b"new fixture").unwrap();
    }
    let original = fs::read(scratch.0.join("UserOptions.ini")).unwrap();
    let source = fs::read(scratch.0.join("ClientConfig.ini")).unwrap();
    let plan = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        Some(&overlay),
        LaunchSettings {
            width: 1280,
            height: 720,
        },
        &mut LocalLoginStub,
    )
    .unwrap();
    // Building a plan is the dry run; it must not write.
    assert_eq!(
        original,
        fs::read(scratch.0.join("UserOptions.ini")).unwrap()
    );
    assert!(!scratch.0.join("Baseline.ini").exists());
    let args = &plan.arguments;
    assert!(args.contains(&"inifile=Baseline.ini".to_owned()));
    assert!(args.contains(&"Server=127.0.0.1:20042".to_owned()));
    assert!(args.contains(&"SessionId=local-session".to_owned()));
    assert!(!args.iter().any(|s| {
        ["guid=", "ticket=", "key=", "cassessionid="]
            .iter()
            .any(|p| s.to_lowercase().starts_with(p))
    }));
    let backup = plan.apply().unwrap();
    assert_eq!(original, fs::read(backup.join("UserOptions.ini")).unwrap());
    assert_eq!(
        source,
        fs::read(scratch.0.join("ClientConfig.ini")).unwrap()
    );
    let baseline = fs::read_to_string(scratch.0.join("Baseline.ini")).unwrap();
    assert!(baseline.contains("Server=127.0.0.1:20042"));
    assert!(baseline.contains("IndirectServerAddress=http://127.0.0.1:"));
    assert!(!baseline.contains("IndirectServerAddress=http://127.0.0.1:9/"));
    assert!(baseline.contains("Address=127.0.0.1"));
    let options = fs::read_to_string(scratch.0.join("UserOptions.ini")).unwrap();
    assert!(options.contains("WindowedWidth=1280\nWindowedHeight=720"));
    assert!(options.contains("Mode=Windowed\nMaximized=0\nMaster=0.000000"));
    for name in ["en_us_data.dat", "en_us_data.dir"] {
        assert_eq!(
            fs::read(scratch.0.join("locale").join(name)).unwrap(),
            b"new fixture"
        );
        assert_eq!(
            fs::read(backup.join("locale").join(name)).unwrap(),
            b"old fixture"
        );
    }
    assert!(plan.apply().is_err()); // Exclusive backup directory; no accidental replay.
}

#[test]
fn invalid_config_or_target_never_changes_files() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("Baseline.ini"), b"preserve").unwrap();
    fs::write(scratch.0.join("ClientConfig.ini"), "Server=a:1\nServer=b:2").unwrap();
    assert!(
        LaunchPlan::build(
            &scratch.0,
            "127.0.0.1:20042",
            None,
            LaunchSettings::default(),
            &mut LocalLoginStub
        )
        .is_err()
    );
    assert_eq!(
        fs::read(scratch.0.join("Baseline.ini")).unwrap(),
        b"preserve"
    );
    assert!(!scratch.0.join(".friend-launcher-backups").exists());
}

#[test]
fn hard_link_target_is_refused() {
    let scratch = Scratch::new();
    let target = scratch.0.join("Baseline.ini");
    fs::hard_link(scratch.0.join("ClientConfig.ini"), &target).unwrap();
    assert!(
        LaunchPlan::build(
            &scratch.0,
            "127.0.0.1:20042",
            None,
            LaunchSettings::default(),
            &mut LocalLoginStub
        )
        .is_err()
    );
}

#[test]
fn arguments_reject_batch_commands_and_missing_ini() {
    let scratch = Scratch::new();
    for line in [
        "FreeRealms.exe inifile=x & calc.exe",
        "FreeRealms.exe Country=US",
        "FreeRealms.exe inifile=x\nFreeRealms.exe inifile=y",
        "FreeRealms.exe inifile=\"unterminated",
    ] {
        fs::write(scratch.0.join("FreeRealms.bat"), line).unwrap();
        assert!(
            LaunchPlan::build(
                &scratch.0,
                "127.0.0.1:20042",
                None,
                LaunchSettings::default(),
                &mut LocalLoginStub
            )
            .is_err(),
            "{line}"
        );
    }
}

#[test]
fn omitted_overlay_and_stale_plan_preserve_local_files() {
    let scratch = Scratch::new();
    let plan = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        None,
        LaunchSettings::default(),
        &mut LocalLoginStub,
    )
    .unwrap();
    assert_eq!(
        plan.changed_files().collect::<Vec<_>>(),
        ["Baseline.ini", "UserOptions.ini"]
    );
    fs::write(scratch.0.join("UserOptions.ini"), b"changed after planning").unwrap();
    assert!(plan.apply().is_err());
    assert!(!scratch.0.join("Baseline.ini").exists());
    assert!(!scratch.0.join(".friend-launcher-backups").exists());
    assert_eq!(
        fs::read(scratch.0.join("locale/en_us_data.dat")).unwrap(),
        b"old fixture"
    );
    for token in ["", "contains spaces", "line\nbreak", "a=b", "unicode-☃"] {
        assert!(SessionToken::new(token.into()).is_err());
    }
    assert!(
        LaunchSettings {
            width: 1,
            height: u32::MAX
        }
        .validate()
        .is_err()
    );
}

#[cfg(windows)]
#[test]
fn failed_replace_rolls_back_completed_changes() {
    let scratch = Scratch::new();
    let options = scratch.0.join("UserOptions.ini");
    let original = fs::read(&options).unwrap();
    let plan = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        None,
        LaunchSettings::default(),
        &mut LocalLoginStub,
    )
    .unwrap();
    let mut permissions = fs::metadata(&options).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&options, permissions.clone()).unwrap();
    let result = plan.apply();
    permissions.set_readonly(false);
    fs::set_permissions(&options, permissions).unwrap();
    assert!(result.is_err());
    assert!(!scratch.0.join("Baseline.ini").exists());
    assert_eq!(original, fs::read(&options).unwrap());
    assert_eq!(
        original,
        fs::read(plan.backup.join("UserOptions.ini")).unwrap()
    );
}

#[cfg(windows)]
#[test]
fn junction_locale_is_refused() {
    use std::process::Command;
    let scratch = Scratch::new();
    let original = scratch.0.join("locale-original");
    fs::rename(scratch.0.join("locale"), &original).unwrap();
    // Fixed command on generated test-owned paths only; never used in launcher.
    let result = Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(scratch.0.join("locale"))
        .arg(&original)
        .output()
        .unwrap();
    assert!(result.status.success());
    let result = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        None,
        LaunchSettings::default(),
        &mut LocalLoginStub,
    );
    fs::remove_dir(scratch.0.join("locale")).unwrap();
    assert!(result.is_err());
    assert_eq!(
        fs::read(original.join("en_us_data.dat")).unwrap(),
        b"old fixture"
    );
}

/// One GET as the client's asset transport sends it; returns (status line, body).
fn get(url: &str, target: &str) -> (String, Vec<u8>) {
    let address = url
        .strip_prefix("http://")
        .and_then(|rest| rest.strip_suffix('/'))
        .unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: {address}\r\nAccept: */*\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(response[..end].to_vec()).unwrap();
    let length = format!("Content-Length: {}", response.len() - end - 4);
    assert!(head.lines().any(|line| line == length), "{head}");
    (
        head.lines().next().unwrap().to_owned(),
        response[end + 4..].to_vec(),
    )
}

#[test]
fn launched_client_gets_its_manifest_crc_and_assets_from_the_responder() {
    let scratch = Scratch::new();
    let assets = scratch.0.join("indirect-assets");
    fs::create_dir(&assets).unwrap();
    fs::write(
        assets.join("sg_bixie_fence_tall_01_lod0.dme.z"),
        b"\xa1\xb2\xc3\xd4 envelope",
    )
    .unwrap();
    fs::write(assets.join("hsg_table_lamp_01.dds.z"), b"lamp envelope").unwrap();
    fs::write(assets.join("Bear Vinegolem.gfx"), b"spaced").unwrap();
    let plan = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        None,
        LaunchSettings::default(),
        &mut LocalLoginStub,
    )
    .unwrap();
    plan.apply().unwrap();
    // The client reads the address from Baseline.ini, so the test does too.
    let baseline = fs::read_to_string(scratch.0.join("Baseline.ini")).unwrap();
    let url = baseline
        .lines()
        .find_map(|line| line.strip_prefix("IndirectServerAddress="))
        .unwrap()
        .to_owned();
    let ok = "HTTP/1.0 200 OK".to_owned();
    // An idle connection holds up no request (the read timeout is 5 s).
    let address = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    let idle = TcpStream::connect(address).unwrap();
    let started = std::time::Instant::now();
    // zlib.crc32(b"123456789") is 0xCBF43926 = 3421780262, as the harness computes it.
    assert_eq!(
        get(&url, "/manifest.crc"),
        (ok.clone(), b"3421780262,9\r\n".to_vec())
    );
    assert_eq!(
        get(&url, "/960/sg_bixie_fence_tall_01_lod0.dme.z?3348701178"),
        (ok.clone(), b"\xa1\xb2\xc3\xd4 envelope".to_vec())
    );
    // Request forms as run 20261021T193008Z logged them.
    assert_eq!(
        get(&url, "/331/hsg_table_lamp_01.dds.z?3968845593"),
        (ok.clone(), b"lamp envelope".to_vec())
    );
    // The client sends manifest names unescaped (0x00d30100), spaces included.
    assert_eq!(
        get(&url, "/417/Bear Vinegolem.gfx?1"),
        (ok, b"spaced".to_vec())
    );
    let missing = get(&url, "/029/hsg_wall_sconce_02_stand.gr2.z?1");
    assert_eq!(missing, ("HTTP/1.0 404 Not Found".to_owned(), Vec::new()));
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    drop((idle, plan));
    // Dropping the responder closes its listener; nothing keeps serving.
    assert!((0..100).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        TcpStream::connect(address).is_err()
    }));
}

#[test]
fn responder_refuses_requests_outside_the_asset_folder() {
    let scratch = Scratch::new();
    let assets = scratch.0.join("indirect-assets");
    fs::create_dir_all(assets.join("sub")).unwrap();
    fs::write(assets.join("sub/nested.z"), b"nested").unwrap();
    fs::write(scratch.0.join("secret.z"), b"outside").unwrap();
    // A hard link inside the folder can alias any file this user can read.
    fs::hard_link(scratch.0.join("secret.z"), assets.join("linked.z")).unwrap();
    let plan = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        None,
        LaunchSettings::default(),
        &mut LocalLoginStub,
    )
    .unwrap();
    let url = plan.assets.url.clone();
    let absolute = scratch.0.join("secret.z").display().to_string();
    let forward = absolute.replace('\\', "/");
    let targets = [
        "/1/../secret.z".to_owned(),
        "/1/..\\secret.z".into(),
        "/1/..%2fsecret.z".into(),
        "/1/..%5csecret.z".into(),
        "/1/%2e%2e%2fsecret.z".into(),
        "/1/%2e%2e".into(),
        "/../secret.z".into(),
        "/1/../../ClientConfig.ini".into(),
        "/1/sub/nested.z".into(),
        "/1/sub%2fnested.z".into(),
        "/1/sub\\nested.z".into(),
        "/x/sub".into(),
        "//secret.z".into(),
        "/1/".into(),
        "/1/.".into(),
        "/1/..".into(),
        "/1/NUL".into(),
        "/1/secret.z::$DATA".into(),
        format!("/1/{absolute}"),
        format!("/{forward}"),
        format!("/1/{forward}"),
        "/1/C:%5CWindows%5Cwin.ini".into(),
        "http://127.0.0.1/manifest.crc".into(),
        "/ClientConfig.ini".into(),
        "/1/linked.z".into(),
    ];
    for target in &targets {
        let refused = ("HTTP/1.0 404 Not Found".to_owned(), Vec::new());
        assert_eq!(get(&url, target), refused, "{target}");
    }
    // Only GET is served.
    let address = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .write_all(b"POST /manifest.crc HTTP/1.1\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(
        response.starts_with("HTTP/1.0 404 Not Found\r\n"),
        "{response}"
    );
    // Without the client's own manifest nothing can answer manifest.crc and the
    // client would exit with code 27, so preparation refuses.
    fs::remove_file(scratch.0.join("Assets_manifest.txt")).unwrap();
    assert!(
        LaunchPlan::build(
            &scratch.0,
            "127.0.0.1:20042",
            None,
            LaunchSettings::default(),
            &mut LocalLoginStub
        )
        .is_err()
    );
}

#[cfg(windows)]
#[test]
fn junction_asset_folder_is_refused() {
    use std::process::Command;
    let scratch = Scratch::new();
    let outside = scratch.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.z"), b"outside").unwrap();
    // Fixed command on generated test-owned paths only; never used in launcher.
    let result = Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(scratch.0.join("indirect-assets"))
        .arg(&outside)
        .output()
        .unwrap();
    assert!(result.status.success());
    let plan = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        None,
        LaunchSettings::default(),
        &mut LocalLoginStub,
    )
    .unwrap();
    let response = get(&plan.assets.url, "/1/secret.z");
    fs::remove_dir(scratch.0.join("indirect-assets")).unwrap();
    // The file is regular, but it resolves outside the folder.
    assert_eq!(response, ("HTTP/1.0 404 Not Found".to_owned(), Vec::new()));
}

#[test]
fn trickled_request_cannot_hold_a_slot() {
    let scratch = Scratch::new();
    let plan = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        None,
        LaunchSettings::default(),
        &mut LocalLoginStub,
    )
    .unwrap();
    let address = plan.assets.url.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(address.trim_end_matches('/')).unwrap();
    // Waiting for the reply paces the sender: one byte per 400 ms never trips
    // a per-read timeout, and the head never ends, so only a deadline closes it.
    stream
        .set_read_timeout(Some(Duration::from_millis(400)))
        .unwrap();
    let start = Instant::now();
    let head = b"GET /manifest.crc HTTP/1.1\r\nX: ";
    let closed = head
        .iter()
        .chain(std::iter::repeat(&b'a'))
        .take_while(|_| start.elapsed() < Duration::from_secs(10))
        .any(|byte| {
            if stream.write_all(&[*byte]).is_err() {
                return true;
            }
            match stream.read(&mut [0; 64]) {
                Ok(0) => true,
                Ok(_) => panic!("a reply to an unfinished request"),
                Err(e) => !matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock),
            }
        });
    assert!(closed && start.elapsed() < Duration::from_secs(7));
}

#[test]
fn responder_closes_connections_past_its_cap() {
    let scratch = Scratch::new();
    let plan = LaunchPlan::build(
        &scratch.0,
        "127.0.0.1:20042",
        None,
        LaunchSettings::default(),
        &mut LocalLoginStub,
    )
    .unwrap();
    let url = plan.assets.url.clone();
    let address = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    let reply = || {
        let mut stream = TcpStream::connect(address).unwrap();
        let _ = stream.write_all(b"GET /manifest.crc HTTP/1.1\r\n\r\n");
        let mut reply = Vec::new();
        let _ = stream.read_to_end(&mut reply);
        reply
    };
    // Idle connections hold every slot (each waits up to 5 s for a request).
    let idle: Vec<_> = (0..crate::assets::CONNECTIONS)
        .map(|_| TcpStream::connect(address).unwrap())
        .collect();
    assert_eq!(reply(), b"");
    // Closed connections free their slots and service resumes.
    drop(idle);
    assert!((0..100).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        reply().starts_with(b"HTTP/1.0 200 OK\r\n")
    }));
}
