//! Server browser: the realm directory (launcher repo `directory/`). Hosts announce a listed realm about
//! once a minute, signed by the realm's listing key; everyone reads the list. The directory checks each
//! realm through its login broker (AUTH2 type 3, server v0.1.5+) before listing it.

use crate::Result;
use crate::patch::{Url, fetch_bytes, post_json};
use ed25519_dalek::{Signer, SigningKey};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

/// The directory this launcher uses unless launcher.json names another (`directory_url`).
pub const DIRECTORY_URL: &str = "https://osfr-directory-719499959539.us-central1.run.app";

/// One listed realm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    pub name: String,
    pub description: String,
    /// The realm's login address (`ip:20042`), as Join takes it.
    pub address: String,
    pub broker: String,
    /// The realm's server identity (hex), proven to the directory by the realm itself.
    pub identity: String,
}

/// What a host announces; the directory checks it against the realm's broker.
pub struct Announce<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub address: &'a str,
    pub broker: &'a str,
    pub identity: &'a str,
}

/// The listed realms, newest list from the directory at `base`.
pub fn servers(base: &str) -> Result<Vec<Listing>> {
    let url = Url::parse(&format!("{}/v1/servers", base.trim_end_matches('/')))?;
    let body = fetch_bytes(&url, 1 << 20, &AtomicBool::new(false))?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "The server list is unreadable.".to_owned())?;
    let text = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned()
    };
    Ok(value
        .get("servers")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .map(|row| Listing {
                    name: text(row, "name"),
                    description: text(row, "description"),
                    address: text(row, "address"),
                    broker: text(row, "broker"),
                    identity: text(row, "identity"),
                })
                .filter(|l| !l.address.is_empty() && l.identity.len() == 64)
                .collect()
        })
        .unwrap_or_default())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes the directory verifies (`directory.py` `announce_message`).
fn announce_message(a: &Announce, key_hex: &str, time: u64) -> Vec<u8> {
    let fields = [
        a.name,
        a.description,
        a.address,
        a.broker,
        a.identity,
        key_hex,
    ];
    let mut text = fields.join("\n");
    text.push_str(&format!("\n{time}"));
    [b"osfr-announce-v1\n".as_slice(), text.as_bytes()].concat()
}

/// List (or keep listed) a realm; the directory's error message on refusal.
pub fn announce(base: &str, key: &SigningKey, a: &Announce) -> Result<()> {
    let key_hex = hex(key.verifying_key().as_bytes());
    let time = now();
    let sig = hex(&key.sign(&announce_message(a, &key_hex, time)).to_bytes());
    let body = serde_json::json!({
        "name": a.name, "description": a.description, "address": a.address, "broker": a.broker,
        "identity": a.identity, "listing_key": key_hex, "time": time, "sig": sig,
    });
    send(base, "/v1/announce", &body)
}

/// Take a realm off the list (when hosting stops).
pub fn delist(base: &str, key: &SigningKey, identity: &str) -> Result<()> {
    let time = now();
    let message = format!("osfr-delist-v1\n{identity}\n{time}");
    let sig = hex(&key.sign(message.as_bytes()).to_bytes());
    send(
        base,
        "/v1/delist",
        &serde_json::json!({"identity": identity, "time": time, "sig": sig}),
    )
}

fn send(base: &str, path: &str, body: &serde_json::Value) -> Result<()> {
    let url = Url::parse(&format!("{}{path}", base.trim_end_matches('/')))?;
    let (status, reply) = post_json(&url, body.to_string().as_bytes())?;
    if status == 200 {
        return Ok(());
    }
    let error = serde_json::from_slice::<serde_json::Value>(&reply)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned));
    Err(error.unwrap_or_else(|| format!("The server list answered HTTP {status}.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signed_announce_matches_the_directory_format() {
        // directory.py: b"osfr-announce-v1\n" + "\n".join([name, description, address, broker,
        // identity, listing_key, str(time)]).
        let a = Announce {
            name: "Housing Party",
            description: "Come decorate",
            address: "203.0.113.7:20042",
            broker: "203.0.113.7:20043",
            identity: "ab",
        };
        assert_eq!(
            announce_message(&a, "cd", 1700000000),
            b"osfr-announce-v1\nHousing Party\nCome decorate\n203.0.113.7:20042\n203.0.113.7:20043\nab\ncd\n1700000000"
        );
    }

    #[test]
    #[ignore = "live directory"]
    fn live_directory_lists_and_refuses_a_realm_it_cannot_reach() {
        assert!(servers(DIRECTORY_URL).is_ok());
        let key = SigningKey::from_bytes(&[5; 32]);
        let a = Announce {
            name: "Unreachable Test",
            description: "",
            address: "1.1.1.1:20042",
            broker: "1.1.1.1:20043",
            identity: &"ab".repeat(32),
        };
        let refused = announce(DIRECTORY_URL, &key, &a).unwrap_err();
        assert!(refused.contains("did not answer"), "{refused}");
    }
}
