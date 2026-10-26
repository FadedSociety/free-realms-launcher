//! Synthetic profiles only. Each test uses its own child process and APPDATA.
use osfr_launcher::{
    auth::{self, AuthError, Profile},
    key_command,
};
use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

// Environment changes are process-wide: never mutate APPDATA in parallel tests.
fn isolated(case: &str) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "w100-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", case, "--nocapture"])
        .env("W100_CHILD", "1")
        .env("APPDATA", &root)
        .output()
        .unwrap();
    let success = result.status.success();
    if !success {
        panic!(
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let target = root.canonicalize().unwrap();
    assert!(
        target.starts_with(
            PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .canonicalize()
                .unwrap()
        )
    );
    fs::remove_dir_all(target).unwrap();
}

fn child() -> bool {
    std::env::var_os("W100_CHILD").is_some()
}

fn profile(endpoint: SocketAddr) -> Profile {
    Profile::new(
        "test-realm",
        "Test realm",
        &endpoint.to_string(),
        [42; 32],
        "acct_001",
    )
    .unwrap()
}

#[test]
fn key_round_trip_location_permissions_and_cli() {
    if !child() {
        return isolated("key_round_trip_location_permissions_and_cli");
    }
    let p = profile("127.0.0.1:1".parse().unwrap());
    let public = key_command(["--new-key", "test-realm"]).unwrap().unwrap();
    assert_eq!(public.len(), 64);
    assert!(public.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(key_command(["--new-key", "test-realm"]).unwrap().is_err());
    auth::save_profile(&p).unwrap();
    assert_eq!(auth::load_profile("test-realm").unwrap(), p);
    assert_eq!(
        key_command(["--print-public-key", "test-realm"])
            .unwrap()
            .unwrap(),
        public
    );
    assert!(key_command(["--new-key", "../escape"]).unwrap().is_err());
    assert!(
        key_command(["--new-key", "test-realm", "extra"])
            .unwrap()
            .is_err()
    );
    assert!(key_command(["--other"]).is_none());
    let directory = auth::profile_directory("test-realm").unwrap();
    assert_eq!(
        directory,
        PathBuf::from(std::env::var_os("APPDATA").unwrap())
            .join("FreeRealmsExpanded/profiles/test-realm")
    );
    let key = directory.join("key.ed25519");
    assert_eq!(fs::metadata(&key).unwrap().len(), 32);
    #[cfg(windows)]
    {
        let result = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command",
            "$a = Get-Acl -LiteralPath $env:W100_KEY; if (!$a.AreAccessRulesProtected -or $a.Access.Count -ne 1 -or $a.Access[0].IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -ne 'S-1-3-4') { exit 1 }"])
            .env("W100_KEY", &key).output().unwrap();
        assert!(result.status.success(), "ACL check failed");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let before = fs::read(&key).unwrap();
    assert!(auth::new_key("test-realm").is_err());
    assert_eq!(fs::read(&key).unwrap(), before);
    fs::write(&key, [0; 33]).unwrap();
    assert_eq!(
        auth::print_public_key("test-realm").unwrap_err(),
        AuthError::KeyStorage
    );
    for id in ["..", "C:escape", "CON", "test.", "a/b", "a\\b"] {
        assert!(auth::profile_directory(id).is_err());
    }
    assert!(Profile::new("ok", "Test", "192.0.2.1:1", [1; 32], "acct_001").is_err());
    assert!(Profile::new("ok", "Test", "localhost:1", [1; 32], "acct_001").is_err());
}

fn request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut size = [0; 4];
    stream.read_exact(&mut size).unwrap();
    let size = u32::from_le_bytes(size) as usize;
    assert!(size <= 192);
    let mut body = vec![0; size];
    stream.read_exact(&mut body).unwrap();
    body
}

fn challenge(id: [u8; 32]) -> [u8; 69] {
    let mut reply = [0; 69];
    reply[0] = 1;
    reply[1..33].copy_from_slice(&id);
    reply[33..65].fill(9);
    reply[65..].copy_from_slice(&60u32.to_le_bytes());
    reply
}

#[test]
fn wrong_pin_sends_no_proof_canary() {
    if !child() {
        return isolated("wrong_pin_sends_no_proof_canary");
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap());
    auth::save_profile(&p).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(request(&mut stream), b"AUTH2\x01\x08\0\0\0acct_001");
        stream.write_all(&challenge([43; 32])).unwrap();
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            match listener.accept() {
                Ok(_) => panic!("proof connection sent despite wrong server pin"),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    assert!(matches!(
        auth::login(&p),
        Err(AuthError::WrongServerIdentity)
    ));
    server.join().unwrap();
}

#[test]
fn refused_proof_is_generic_and_never_retried() {
    if !child() {
        return isolated("refused_proof_is_generic_and_never_retried");
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap());
    auth::save_profile(&p).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        stream.write_all(&challenge([42; 32])).unwrap();
        drop(stream);
        let (mut stream, _) = listener.accept().unwrap();
        let body = request(&mut stream);
        assert_eq!(body.len(), 134);
        assert_eq!(&body[..6], b"AUTH2\x02");
        stream.write_all(&[0; 69]).unwrap();
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        thread::sleep(Duration::from_millis(150));
        assert!(listener.accept().is_err(), "refused proof retried");
    });
    assert!(matches!(auth::login(&p), Err(AuthError::Refused)));
    assert_eq!(
        AuthError::Refused.to_string(),
        "Login refused. Check key enrolment or try again later."
    );
    server.join().unwrap();
}

#[test]
fn slow_reply_hits_total_deadline() {
    if !child() {
        return isolated("slow_reply_hits_total_deadline");
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap());
    auth::save_profile(&p).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        for b in challenge([42; 32]) {
            if stream.write_all(&[b]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    let start = Instant::now();
    assert!(matches!(auth::login(&p), Err(AuthError::Timeout)));
    assert!(start.elapsed() < Duration::from_secs(5));
    server.join().unwrap();
}

struct Broker(Child);
impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn real_broker_enrol_generated_key_and_receive_token() {
    if !child() {
        return isolated("real_broker_enrol_generated_key_and_receive_token");
    }
    use std::io::{BufRead, BufReader};
    // The real login broker: set OSFR_SERVER_BIN to a folder holding osfr-login.exe (built from the
    // server project); without it this test is skipped.
    let Some(binaries) = std::env::var_os("OSFR_SERVER_BIN").map(PathBuf::from) else {
        eprintln!("skip: set OSFR_SERVER_BIN to run the real broker test");
        return;
    };
    let root = PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("broker");
    fs::create_dir_all(root.join("CharacterCreate")).unwrap();
    let mut model = vec!["0"; 17];
    model[0] = "1";
    model[5] = "1";
    model[13] = "1";
    fs::write(root.join("Models.txt"), model.join("^")).unwrap();
    for name in [
        "HeadMappings.txt",
        "HairMappings.txt",
        "ModelCustomizationMappings.txt",
        "FacePaintMappings.txt",
        "SkinToneMappings.txt",
        "ItemDefinitions.txt",
    ] {
        let contents = if name == "ItemDefinitions.txt" {
            let mut row = vec!["0"; 51];
            row[0] = "1";
            row.join("^")
        } else {
            "1^synthetic.adr^".into()
        };
        fs::write(root.join("CharacterCreate").join(name), contents).unwrap();
    }
    fs::write(root.join("key"), [9; 32]).unwrap();
    fs::write(root.join("server-id"), [42; 32]).unwrap();
    let public = auth::new_key("test-realm").unwrap();
    let initial = auth::new_key("bootstrap").unwrap();
    let store = root.join("accounts.json");
    let provision = |args: &[&str]| {
        let output = Command::new(binaries.join("osfr-account.exe"))
            .args(args)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "operator enrolment failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    provision(&["create", "--handle", "acct_001", "--public-key", &initial]);
    provision(&["add-key", "--account-id", "1", "--public-key", &public]);
    let child = Command::new(binaries.join("osfr-login.exe"))
        .args([
            "--bind",
            "127.0.0.1:0",
            "--auth-bind",
            "127.0.0.1:0",
            "--gateway",
            "127.0.0.1:20260",
            "--seconds",
            "15",
        ])
        .arg("--resources")
        .arg(&root)
        .arg("--identity")
        .arg(root.join("identity.json"))
        .arg("--key-file")
        .arg(root.join("key"))
        .arg("--handoff-key-file")
        .arg(root.join("key"))
        .arg("--account-store")
        .arg(&store)
        .arg("--server-id-file")
        .arg(root.join("server-id"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut broker = Broker(child);
    let stdout = broker.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let output_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(6);
    let endpoint = loop {
        let line = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("scratch broker did not listen in time");
        let event: serde_json::Value = serde_json::from_str(&line).unwrap();
        if event["event"] == "auth_listening" {
            break event["address"].as_str().unwrap().parse().unwrap();
        }
    };
    let p = profile(endpoint);
    auth::save_profile(&p).unwrap();
    let token = auth::login(&p).expect("real broker rejected enrolled generated key");
    // Token is deliberately not Debug/Display. A successful typed result proves
    // the strict 32 lowercase-hex / zero-tail reply check accepted this broker.
    drop(token);
    // Unknown and locked accounts share the same refusal kind/message.
    let unknown = Profile::new(
        "unknown",
        "Unknown fixture",
        &endpoint.to_string(),
        [42; 32],
        "acct_999",
    )
    .unwrap();
    auth::save_profile(&unknown).unwrap();
    assert!(matches!(auth::login(&unknown), Err(AuthError::Refused)));
    drop(broker);
    output_reader.join().unwrap();
}

#[test]
fn malformed_reply_and_unreachable_server_fail_closed() {
    if !child() {
        return isolated("malformed_reply_and_unreachable_server_fail_closed");
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let p = profile(endpoint);
    auth::save_profile(&p).unwrap();
    drop(listener);
    assert!(matches!(auth::login(&p), Err(AuthError::ServerUnreachable)));
    let listener = TcpListener::bind(endpoint).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        let mut bad = [0; 69];
        bad[0] = 2;
        stream.write_all(&bad).unwrap();
    });
    assert!(matches!(auth::login(&p), Err(AuthError::InvalidReply)));
    server.join().unwrap();
    let mut changed = p.clone();
    changed.server_identity = [99; 32];
    assert!(matches!(
        auth::login(&changed),
        Err(AuthError::InvalidProfile)
    ));
}
