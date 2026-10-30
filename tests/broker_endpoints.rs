//! Broker policy (L075: public realms allowed); no profile files or keys are created.
use osfr_launcher::auth::{AuthError, Profile};

#[test]
fn broker_profiles_accept_loopback_private_and_public_literal_addresses() {
    for endpoint in [
        "127.0.0.1:20043", "[::1]:20043", "10.0.0.1:20043",
        "172.16.0.1:20043", "172.31.255.254:20043", "192.168.1.1:20043",
        "100.64.0.1:20043", "100.127.255.254:20043", "[fc00::1]:20043",
        "[fdff::1]:20043", "8.8.8.8:20043", "192.0.2.1:20043", "[2001:4860::1]:20043",
    ] {
        assert!(Profile::new("fixture", "Fixture realm", endpoint, [42; 32], "acct_001").is_ok(), "{endpoint}");
    }
}

#[test]
fn broker_profiles_refuse_wildcard_link_local_multicast_dns_and_invalid_ports() {
    for endpoint in [
        "224.0.0.1:20043", "255.255.255.255:20043", "169.254.1.1:20043",
        "[ff02::1]:20043", "[fe80::1]:20043",
        "0.0.0.0:20043", "[::]:20043", "localhost:20043", "127.0.0.1:0",
        "10.0.0.1:65536", "10.0.0.1:20043/path",
    ] {
        assert_eq!(Profile::new("fixture", "Fixture realm", endpoint, [42; 32], "acct_001"), Err(AuthError::InvalidProfile), "{endpoint}");
    }
}
