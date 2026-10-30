//! The local server from GitHub Releases: installed on first use, checked on each start.
//! Built on `patch.rs` (URL rules, resumable size+SHA-256 verified downloads, manifest checks).
//! Remote bytes are untrusted until they match the release manifest; nothing here executes them.
//! The manifest must carry an Ed25519 signature (`server-manifest.json.sig`, 64 raw bytes) from
//! the release key pinned below, so control of the GitHub repository alone cannot push binaries
//! to existing installs. The private key never leaves the maintainer's machine.
//!
//! Inside `server/`: files named by the manifest are managed. `state/` is never touched (a
//! manifest naming it is refused). `keys/` and `*.key` files are created when missing and never
//! overwritten. Hidden names (manifest paths cannot start with a dot) hold launcher state:
//! `.osfr-release.json` (the installed manifest, written last), `.update-stage/` (verified
//! downloads), `.update-old/` (files the swap moved aside) and `.update-journal` (a swap in
//! progress, so a crash can be rolled back).
use crate::{
    Result, bounded_read, hard_link, io_error, no_link,
    patch::{
        Entry, Limits, MANIFEST_LIMIT, Manifest, Progress, Url, backoff, cancelled, download,
        fetch_bytes, matches, parse, root, target, valid_path,
    },
};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

pub const REPO: &str = "FadedSociety/free-realms-expanded";
const MANIFEST_ASSET: &str = "server-manifest.json";
const SIGNATURE_ASSET: &str = "server-manifest.json.sig";
/// The public half of the release signing key (Ed25519).
pub const RELEASE_KEY: [u8; 32] = [
    0x99, 0x8a, 0x74, 0x68, 0xc4, 0x21, 0xbd, 0xad, 0x44, 0x99, 0x19, 0x72, 0xd3, 0xa0, 0x1a, 0x90,
    0x61, 0x00, 0xe1, 0xaa, 0x86, 0x3b, 0xb9, 0x08, 0xbf, 0x44, 0x8a, 0x13, 0xfa, 0xcb, 0x37, 0xcd,
];
/// Sane caps: GitHub itself refuses release assets over 2 GiB.
const LIMITS: Limits = Limits {
    files: 5_000,
    file: 2 << 30,
    total: 4 << 30,
};
const API_LIMIT: u64 = 2 * 1024 * 1024;
const MARKER: &str = ".osfr-release.json";
const STAGE: &str = ".update-stage";
const OLD: &str = ".update-old";
const JOURNAL: &str = ".update-journal";

/// Where releases come from. Only GitHub over HTTPS, or literal-loopback HTTP for tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSource {
    /// Returns JSON with the latest release's `tag_name`.
    pub api_url: String,
    /// Assets live at `<download_base>/<tag>/<asset>`.
    pub download_base: String,
    /// Ed25519 key the manifest signature must verify against.
    pub key: [u8; 32],
}

impl Default for ReleaseSource {
    fn default() -> Self {
        Self {
            api_url: format!("https://api.github.com/repos/{REPO}/releases/latest"),
            download_base: format!("https://github.com/{REPO}/releases/download"),
            key: RELEASE_KEY,
        }
    }
}

#[derive(Debug)]
pub struct Outcome {
    pub tag: String,
    /// False when the installed server already matched the latest release.
    pub installed: bool,
    pub files_downloaded: usize,
    pub bytes_downloaded: u64,
}

/// One path the swap touches: `had` = it exists now (moved aside), `install` = new bytes are staged.
struct Change {
    path: String,
    had: bool,
    install: bool,
}

fn valid_name(text: &str, max: usize) -> bool {
    text.len() <= max
        && text.starts_with(|c: char| c.is_ascii_alphanumeric())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn under_state(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == "state" || lower.starts_with("state/")
}

/// Player-owned once present: installed if missing, never overwritten or removed.
fn create_only(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == "keys"
        || lower.starts_with("keys/")
        || lower.ends_with(".key")
        || lower.ends_with(".pem")
}

fn check(manifest: &Manifest, tag: &str) -> Result<()> {
    if manifest.tag.as_deref() != Some(tag) {
        return Err("The server manifest is for a different release.".into());
    }
    for entry in &manifest.files {
        let named = entry.asset.as_deref().is_some_and(|a| valid_name(a, 100));
        if !named || under_state(&entry.path) {
            return Err("Unsafe asset name or protected path in the server manifest.".into());
        }
    }
    Ok(())
}

/// `manifest` is exactly what the release key signed.
fn verify(key: &[u8; 32], manifest: &[u8], signature: &[u8]) -> Result<()> {
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| "The server manifest signature is malformed.")?;
    VerifyingKey::from_bytes(key)
        .and_then(|key| key.verify_strict(manifest, &Signature::from_bytes(&signature)))
        .map_err(|_| "The server manifest is not signed by the release key.".into())
}

fn attempt<T>(cancel: &AtomicBool, tries: u32, mut run: impl FnMut() -> Result<T>) -> Result<T> {
    for n in 0..tries {
        cancelled(cancel)?;
        match run() {
            Ok(value) => return Ok(value),
            Err(e) if n + 1 == tries => return Err(e),
            Err(_) => backoff(n, cancel)?,
        }
    }
    Err("No attempt was made.".into())
}

fn latest_tag(api: &Url, cancel: &AtomicBool) -> Result<String> {
    #[derive(Deserialize)]
    struct Latest {
        tag_name: String,
    }
    let latest: Latest = serde_json::from_slice(&fetch_bytes(api, API_LIMIT, cancel)?)
        .map_err(|_| "Invalid release information from GitHub.")?;
    if !valid_name(&latest.tag_name, 64) {
        return Err("Unsafe release tag from GitHub.".into());
    }
    Ok(latest.tag_name)
}

/// The installed manifest, if the marker is a real file holding a valid one.
fn installed(server: &Path) -> Option<Manifest> {
    let marker = server.join(MARKER);
    no_link(&marker).ok()?.is_file().then_some(())?;
    parse(&bounded_read(&marker, MANIFEST_LIMIT).ok()?, &LIMITS).ok()
}

/// `root/relative` if it exists as a real file under real directories; `None` if absent.
/// Never creates anything, so a failed update leaves `server/` as it was.
fn existing(root: &Path, relative: &str) -> Result<Option<PathBuf>> {
    let mut path = root.to_path_buf();
    let parts: Vec<_> = relative.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error(e)),
        }
        let metadata = no_link(&path)?;
        let unsafe_entry = if index + 1 == parts.len() {
            !metadata.is_file() || hard_link(&path)?
        } else {
            !metadata.is_dir()
        };
        if unsafe_entry {
            return Err("Unsafe file or folder inside the server folder.".into());
        }
    }
    Ok(Some(path))
}

fn shift(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to.parent().ok_or("Missing parent folder.")?).map_err(io_error)?;
    fs::rename(from, to).map_err(io_error)
}

/// Put back what a swap moved aside and remove what it added. Idempotent. Tries every path.
fn rollback<'a>(server: &Path, lines: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut result = Ok(());
    for line in lines {
        let Some((kind, path)) = line.split_once(' ') else {
            continue;
        };
        if !valid_path(path) || under_state(path) {
            continue;
        }
        let (live, aside) = (server.join(path), server.join(OLD).join(path));
        let step = match kind {
            "M" if aside.is_file() => fs::rename(&aside, &live),
            "A" if live.is_file() => fs::remove_file(&live),
            _ => Ok(()),
        };
        if step.is_err() {
            result = Err("Could not fully restore the previous server files.".to_owned());
        }
    }
    result
}

fn clean(server: &Path, stage: bool) {
    let _ = fs::remove_file(server.join(JOURNAL));
    let _ = fs::remove_dir_all(server.join(OLD));
    if stage {
        let _ = fs::remove_dir_all(server.join(STAGE));
    }
}

/// A crash can leave a swap half done: roll it back, or just clean up if it had committed
/// (the marker already names the journal's tag).
fn recover(server: &Path) -> Result<()> {
    let Ok(text) = fs::read_to_string(server.join(JOURNAL)) else {
        return Ok(());
    };
    let mut lines = text.lines();
    let tag = lines.next().unwrap_or("");
    if installed(server).and_then(|m| m.tag).as_deref() != Some(tag) {
        rollback(server, lines)?;
    }
    clean(server, true);
    Ok(())
}

/// Move the staged files in and the replaced ones aside, then write the marker last.
/// Any failure rolls everything back; a crash is rolled back by [`recover`].
fn swap(server: &Path, changes: &[Change], tag: &str, manifest: &[u8]) -> Result<()> {
    let (stage, old) = (server.join(STAGE), server.join(OLD));
    let _ = fs::remove_dir_all(&old);
    let lines: Vec<String> = changes
        .iter()
        .map(|c| format!("{} {}", if c.had { 'M' } else { 'A' }, c.path))
        .collect();
    let mut journal = File::create(server.join(JOURNAL)).map_err(io_error)?;
    journal
        .write_all(format!("{tag}\n{}\n", lines.join("\n")).as_bytes())
        .and_then(|_| journal.sync_all())
        .map_err(io_error)?;
    drop(journal);
    let temp = server.join(".update-marker");
    let moved = (|| {
        for c in changes {
            if c.had {
                shift(&server.join(&c.path), &old.join(&c.path))?;
            }
            if c.install {
                shift(&stage.join(&c.path), &server.join(&c.path))?;
            }
        }
        fs::write(&temp, manifest).map_err(io_error)?;
        fs::rename(&temp, server.join(MARKER)).map_err(io_error)
    })();
    if let Err(e) = moved {
        let _ = fs::remove_file(&temp);
        rollback(server, lines.iter().map(String::as_str))?;
        clean(server, true);
        return Err(e);
    }
    clean(server, true);
    Ok(())
}

/// Check GitHub for the latest release and bring `server` up to it. `force` re-verifies every
/// file even when the installed tag is current (repair). All-or-nothing: files are staged and
/// verified first, then swapped in; any failure leaves the previous `server/` as it was.
pub fn sync(
    server: &Path,
    source: &ReleaseSource,
    force: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> Result<Outcome> {
    cancelled(cancel)?;
    let api = Url::release(&source.api_url)?;
    let base = source.download_base.trim_end_matches('/');
    Url::release(&format!("{base}/"))?; // the base must obey the same host rules
    let mut status = Progress {
        phase: "Checking",
        ..Default::default()
    };
    progress(&status);
    if server.is_dir() {
        recover(server)?;
    }
    let tag = attempt(cancel, 1, || latest_tag(&api, cancel))?;
    let previous = installed(server);
    if !force && previous.as_ref().and_then(|m| m.tag.as_deref()) == Some(tag.as_str()) {
        return Ok(Outcome {
            tag,
            installed: false,
            files_downloaded: 0,
            bytes_downloaded: 0,
        });
    }
    status.phase = "Update available";
    progress(&status);
    let asset_url = |asset: &str| Url::release(&format!("{base}/{tag}/{asset}"));
    let raw = attempt(cancel, 3, || {
        fetch_bytes(&asset_url(MANIFEST_ASSET)?, MANIFEST_LIMIT, cancel)
    })?;
    let signature = attempt(cancel, 3, || {
        fetch_bytes(&asset_url(SIGNATURE_ASSET)?, 64, cancel)
    })?;
    verify(&source.key, &raw, &signature)?;
    let manifest = parse(&raw, &LIMITS)?;
    check(&manifest, &tag)?;

    // Pass 1: what differs. Read-only on server/; every installed path is validated here.
    fs::create_dir_all(server).map_err(io_error)?;
    let live = root(server)?;
    status.phase = "Verifying";
    let mut wanted: Vec<(&Entry, bool)> = Vec::new();
    for entry in &manifest.files {
        cancelled(cancel)?;
        status.current_file = entry.path.clone();
        progress(&status);
        let have = existing(&live, &entry.path)?;
        let current = match &have {
            Some(path) => create_only(&entry.path) || matches(path, entry, cancel)?,
            None => false,
        };
        if !current {
            wanted.push((entry, have.is_some()));
        }
    }
    status.files_total = wanted.len();
    status.bytes_total = wanted.iter().map(|(e, _)| e.size).sum();

    // Pass 2: download and verify into the staging folder (resumable, reused when still valid).
    let stage = server.join(STAGE);
    fs::create_dir_all(&stage).map_err(io_error)?;
    let staging = root(&stage)?;
    let (mut files, mut bytes, mut done) = (0, 0, 0);
    for (entry, _) in &wanted {
        cancelled(cancel)?;
        status.current_file = entry.path.clone();
        status.bytes_done = done;
        status.phase = "Verifying";
        progress(&status);
        if !matches(&target(&staging, &entry.path)?, entry, cancel)? {
            let url = asset_url(entry.asset.as_deref().unwrap_or_default())?;
            status.phase = "Downloading";
            attempt(cancel, 3, || {
                download(
                    &staging,
                    entry,
                    &url,
                    cancel,
                    |received| {
                        status.bytes_done = done + received;
                        progress(&status);
                    },
                    &mut bytes,
                )
            })?;
            files += 1;
        }
        done += entry.size;
        status.files_done += 1;
    }

    // Pass 3: swap. Files the previous release shipped and this one dropped are moved aside too.
    let mut changes: Vec<Change> = wanted
        .iter()
        .map(|(e, had)| Change {
            path: e.path.clone(),
            had: *had,
            install: true,
        })
        .collect();
    if let Some(previous) = &previous {
        let kept: BTreeSet<_> = manifest
            .files
            .iter()
            .map(|e| e.path.to_ascii_lowercase())
            .collect();
        for e in &previous.files {
            let protected = under_state(&e.path) || create_only(&e.path);
            if !protected
                && !kept.contains(&e.path.to_ascii_lowercase())
                && existing(&live, &e.path)?.is_some()
            {
                changes.push(Change {
                    path: e.path.clone(),
                    had: true,
                    install: false,
                });
            }
        }
    }
    cancelled(cancel)?;
    status.phase = "Installing";
    status.current_file.clear();
    progress(&status);
    swap(server, &changes, &tag, &raw)?;
    status.phase = "Ready";
    progress(&status);
    Ok(Outcome {
        tag,
        installed: true,
        files_downloaded: files,
        bytes_downloaded: bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live check, run by hand (`cargo test -- --ignored live_github`): real TLS to api.github.com,
    /// the github.com download redirect to GitHub's CDN, and the host allow-list, on a public repo.
    #[test]
    #[ignore = "needs network"]
    fn live_github_https_and_redirect() {
        #[derive(Deserialize)]
        struct Release {
            tag_name: String,
            assets: Vec<Asset>,
        }
        #[derive(Deserialize)]
        struct Asset {
            name: String,
            size: u64,
        }
        let cancel = AtomicBool::new(false);
        let repo = "BurntSushi/ripgrep";
        let api = Url::release(&format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        .unwrap();
        let json = fetch_bytes(&api, API_LIMIT, &cancel).unwrap();
        let release: Release = serde_json::from_slice(&json).unwrap();
        let smallest = release.assets.iter().min_by_key(|a| a.size).unwrap();
        let url = Url::release(&format!(
            "https://github.com/{repo}/releases/download/{}/{}",
            release.tag_name, smallest.name
        ))
        .unwrap();
        let bytes = fetch_bytes(&url, 1 << 20, &cancel).unwrap();
        assert_eq!(bytes.len() as u64, smallest.size);
    }
}

/// One line of the Play tab's news panel (redesign 2026-10-30).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewsItem {
    pub title: String,
    pub date: String,
    pub url: String,
}

/// The latest release of the server and of the launcher, newest first, as news for the Play tab.
/// Uses the same GitHub-only HTTPS rules as the server download.
pub fn fetch_news(limit: usize) -> Result<Vec<NewsItem>> {
    let cancel = AtomicBool::new(false);
    let mut items = Vec::new();
    for (repo, label) in [(REPO, "Server"), ("FadedSociety/free-realms-launcher", "Launcher")] {
        let url = Url::release(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
        let body = fetch_bytes(&url, 1 << 20, &cancel)?;
        items.extend(news_items(&body, label)?);
    }
    items.sort_by(|a, b| b.date.cmp(&a.date));
    items.truncate(limit);
    Ok(items)
}

fn news_items(body: &[u8], label: &str) -> Result<Vec<NewsItem>> {
    // `releases/latest` answers one object (lists come chunked, which downloads refuse); both parse.
    let list = match serde_json::from_slice(body)
        .map_err(|_| "GitHub sent unreadable release news.".to_owned())?
    {
        serde_json::Value::Array(list) => list,
        one => vec![one],
    };
    Ok(list
        .iter()
        .filter(|r| r.get("draft").and_then(|v| v.as_bool()) != Some(true))
        .map(|r| {
            let text = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let name = if text("name").is_empty() { text("tag_name") } else { text("name") };
            let url = text("html_url");
            NewsItem {
                title: format!("{label} {name}"),
                date: text("published_at").chars().take(10).collect(),
                url: if url.starts_with("https://github.com/") { url } else { String::new() },
            }
        })
        .collect())
}

#[cfg(test)]
mod news_tests {
    use super::*;

    #[test]
    #[ignore = "live GitHub request"]
    fn live_news_fetch() {
        let t = std::time::Instant::now();
        let news = fetch_news(4);
        println!("{:?} in {:?}", news, t.elapsed());
        assert!(news.is_ok());
    }

    #[test]
    fn news_lists_published_releases_and_keeps_only_github_links() {
        let body = br#"[{"name":"v0.2.3 (early preview)","tag_name":"v0.2.3","published_at":"2026-10-30T22:00:00Z","html_url":"https://github.com/FadedSociety/free-realms-launcher/releases/tag/v0.2.3","draft":false},
            {"name":"","tag_name":"v9","published_at":"2026-10-31T00:00:00Z","html_url":"https://evil.example/x","draft":false},
            {"name":"draft","published_at":"2026-11-01T00:00:00Z","draft":true}]"#;
        let items = news_items(body, "Launcher").unwrap();
        assert_eq!(items.len(), 2, "drafts are not news");
        assert_eq!(items[0].title, "Launcher v0.2.3 (early preview)");
        assert_eq!(items[0].date, "2026-10-30");
        assert_eq!(items[1].title, "Launcher v9", "the tag stands in for an empty name");
        assert!(items[1].url.is_empty(), "only github.com links are shown");
    }
}
