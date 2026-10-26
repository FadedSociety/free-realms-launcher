//! Reconstructed VPN-only broker policy; no profile files or keys are created.
use osfr_launcher::auth::{AuthError, Profile};

#[test]
fn broker_profiles_accept_loopback_and_private_literal_addresses() {
    for endpoint in [
        "127.0.0.1:20043", "[::1]:20043", "10.0.0.1:20043",
        "172.16.0.1:20043", "172.31.255.254:20043", "192.168.1.1:20043",
        "100.64.0.1:20043", "100.127.255.254:20043", "[fc00::1]:20043",
        "[fdff::1]:20043",
    ] {
        assert!(Profile::new("fixture", "Fixture realm", endpoint, [42; 32], "acct_001").is_ok(), "{endpoint}");
    }
}

#[test]
fn broker_profiles_refuse_public_wildcard_dns_and_invalid_ports() {
    for endpoint in [
        "8.8.8.8:20043", "192.0.2.1:20043", "172.15.255.255:20043",
        "172.32.0.1:20043", "100.63.255.255:20043", "100.128.0.1:20043",
        "[2001:4860::1]:20043", "[::ffff:8.8.8.8]:20043", "[fe80::1]:20043",
        "0.0.0.0:20043", "[::]:20043", "localhost:20043", "127.0.0.1:0",
        "10.0.0.1:65536", "10.0.0.1:20043/path",
    ] {
        assert_eq!(Profile::new("fixture", "Fixture realm", endpoint, [42; 32], "acct_001"), Err(AuthError::InvalidProfile), "{endpoint}");
    }
}
