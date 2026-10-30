//! Socket-only checks remain runnable under a token that cannot reopen private
//! profile ACLs. The production-login integration tests still require that gate.
use super::*;
use std::{net::TcpListener, thread};

#[test]
fn fixed_reply_framing_refusals_and_disconnects() {
    for status in [0, 1, 2, 3] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut frame = [0; 17];
            stream.read_exact(&mut frame).unwrap();
            assert_eq!(&frame[..4], &13u32.to_le_bytes());
            assert_eq!(&frame[4..], b"AUTH2\x01\x03\0\0\0abc");
            if status == 3 {
                return;
            }
            let mut reply = [0; 69];
            reply[0] = status;
            stream.write_all(&reply[..11]).unwrap();
            stream.write_all(&reply[11..]).unwrap();
        });
        let result = exchange(address, b"AUTH2\x01\x03\0\0\0abc");
        match status {
            0 => assert!(matches!(result, Err(AuthError::Refused))),
            1 => assert_eq!(result.unwrap()[0], 1),
            2 => assert!(matches!(result, Err(AuthError::InvalidReply))),
            3 => assert!(matches!(result, Err(AuthError::TryLater))),
            _ => unreachable!(),
        }
        server.join().unwrap();
    }
}

#[test]
fn trickled_bytes_cannot_extend_the_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut frame = [0; 17];
        stream.read_exact(&mut frame).unwrap();
        for _ in 0..69 {
            if stream.write_all(&[1]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    let start = Instant::now();
    assert!(matches!(
        exchange(address, b"AUTH2\x01\x03\0\0\0abc"),
        Err(AuthError::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(5));
    server.join().unwrap();
}

#[test]
fn profile_validation_and_cli_reject_paths_and_remote_brokers() {
    for id in ["..", "CON", "COM1", "NUL", "c:escape", "a/b", "a\\b"] {
        assert_eq!(valid_id(id), Err(AuthError::InvalidProfile));
    }
    for endpoint in ["192.0.2.1:1", "localhost:1", "127.0.0.1:0", "[::]:1"] {
        assert!(Profile::new("fixture", "Fixture realm", endpoint, [42; 32], "acct_001").is_err());
    }
    assert!(Profile::new("fixture", "Fixture realm", "[::1]:1", [42; 32], "acct_001").is_ok());
    assert!(crate::key_command(["--new-key"]).unwrap().is_err());
    assert!(
        crate::key_command(["--print-public-key", "fixture", "extra"])
            .unwrap()
            .is_err()
    );
    assert!(crate::key_command(["--other"]).is_none());
}

/// L070 join by address: the realm's answer to a challenge carries its identity, which the
/// player compares (as a fingerprint) with the host's before a profile pins it. Public and
/// malformed broker addresses are refused before any connection.
#[test]
fn fetch_identity_reads_the_challenge_identity_and_refuses_public_brokers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut frame = [0; 24];
        stream.read_exact(&mut frame).unwrap();
        assert_eq!(&frame[4..], b"AUTH2\x01\x0a\0\0\0join-check");
        let mut reply = [0; 69];
        reply[0] = 1;
        reply[1..33].copy_from_slice(&[7; 32]);
        stream.write_all(&reply).unwrap();
    });
    assert_eq!(fetch_identity(&address.to_string()), Ok([7; 32]));
    server.join().unwrap();
    for broker in ["8.8.8.8:20043", "0.0.0.0:20043", "realm.example:20043", ""] {
        assert_eq!(
            fetch_identity(broker),
            Err(AuthError::InvalidProfile),
            "{broker}"
        );
    }
}
