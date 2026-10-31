//! (R) Pinned patch policy, not a recovered Free Realms protocol.
//! Remote bytes remain untrusted until the manifest pin and each file hash pass.
//! Use a private client copy: hostile concurrent same-user filesystem mutation
//! is outside the existing launcher's isolation boundary.
use crate::{Result, hard_link, io_error, no_link, safe_target};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(crate) const MANIFEST_LIMIT: u64 = 4 * 1024 * 1024;
const PART: &str = ".osfr-part";

/// Trusted, local, per-realm settings. The pin must come from the operator,
/// never from the downloaded manifest. Mirrors serve the identical manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PatchSource {
    pub manifest_url: String,
    pub manifest_sha256: String,
    pub mirror_url: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub files_done: usize,
    pub files_total: usize,
    /// Verified files plus the current partial; may decrease after a bad hash.
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current_file: String,
    pub phase: &'static str,
}

#[derive(Debug, Default)]
pub struct PatchReport {
    pub files_downloaded: usize,
    pub bytes_downloaded: u64,
}

/// `tag` and `asset` are used by the GitHub release flow (`release.rs`) only.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    version: u32,
    #[serde(default)]
    pub tag: Option<String>,
    pub files: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub asset: Option<String>,
    /// L070: the community client manifest's XXH64 (seed 0) in place of `sha256`.
    #[serde(skip)]
    pub xxh64: Option<u64>,
}

/// Manifest size caps: file count, one file, all files.
pub(crate) struct Limits {
    pub files: usize,
    pub file: u64,
    pub total: u64,
}
const PATCH_LIMITS: Limits = Limits {
    files: 50_000,
    file: 16 * 1024 * 1024 * 1024,
    total: 128 * 1024 * 1024 * 1024,
};

pub(crate) fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err("Patching cancelled. Partial downloads are kept.".into())
    } else {
        Ok(())
    }
}

pub(crate) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn valid_path(path: &str) -> bool {
    // (R) Portable ASCII filenames, no ADS, device aliases, hidden launcher
    // state or reserved partial names. Slash alone separates components.
    !path.is_empty()
        && path.len() <= 220
        && path.split('/').all(|part| {
            let base = part
                .split('.')
                .next()
                .unwrap_or("")
                .trim_end()
                .to_ascii_uppercase();
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with(['.', ' '])
                && !part.to_ascii_lowercase().ends_with(PART)
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.() ".contains(&b))
                && !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(base.len() == 4
                    && (base.starts_with("COM") || base.starts_with("LPT"))
                    && base.as_bytes()[3].is_ascii_digit())
        })
}

fn manifest(bytes: &[u8], pin: &str) -> Result<Manifest> {
    if !valid_hash(pin) || !sha256(bytes)?.eq_ignore_ascii_case(pin) {
        return Err("Patch manifest does not match the trusted SHA-256 pin.".into());
    }
    parse(bytes, &PATCH_LIMITS)
}

/// Parse and structurally validate manifest JSON (paths, hashes, sizes, overlaps).
/// The caller decides what vouches for the bytes: an operator pin or a release tag.
pub(crate) fn parse(bytes: &[u8], limits: &Limits) -> Result<Manifest> {
    let value: Manifest =
        serde_json::from_slice(bytes).map_err(|_| "Invalid patch manifest JSON.")?;
    if value.version != 1 {
        return Err("Unsupported or empty patch manifest.".into());
    }
    check(&value.files, limits)?;
    Ok(value)
}

/// Every entry's path, hash and size, no duplicate or overlapping paths, and the size limits.
fn check(files: &[Entry], limits: &Limits) -> Result<()> {
    if files.is_empty() || files.len() > limits.files {
        return Err("Unsupported or empty patch manifest.".into());
    }
    let mut names = BTreeSet::new();
    let mut total = 0_u64;
    for entry in files {
        let hashed = entry.xxh64.is_some() || valid_hash(&entry.sha256);
        if !valid_path(&entry.path) || !hashed || entry.size > limits.file {
            return Err("Unsafe patch path, hash or file size in manifest.".into());
        }
        if !names.insert(entry.path.to_ascii_lowercase()) {
            return Err("Duplicate patch path in manifest.".into());
        }
        total = total
            .checked_add(entry.size)
            .ok_or("Patch size overflow.")?;
        if total > limits.total {
            return Err("Patch exceeds the total size limit.".into());
        }
    }
    for name in &names {
        for (index, _) in name.match_indices('/') {
            if names.contains(&name[..index]) {
                return Err("Patch file overlaps another file's directory.".into());
            }
        }
    }
    Ok(())
}

pub(crate) fn root(client: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(client).map_err(io_error)?;
    for ancestor in absolute.ancestors() {
        if !no_link(ancestor)?.is_dir() {
            return Err("Client folder must be a real directory.".into());
        }
    }
    absolute.canonicalize().map_err(io_error)
}

pub(crate) fn target(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut directory = root.to_path_buf();
    let parts: Vec<_> = relative.split('/').collect();
    for component in &parts[..parts.len() - 1] {
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&directory).map_err(io_error)?
            }
            Err(e) => return Err(io_error(e)),
        }
        if !no_link(&directory)?.is_dir() {
            return Err("Unsafe patch parent directory.".into());
        }
    }
    // Existing launch core checks containment, final reparse points and hard links.
    safe_target(root, relative)
}

pub(crate) fn matches(path: &Path, entry: &Entry, cancel: &AtomicBool) -> Result<bool> {
    if !path.try_exists().map_err(io_error)? {
        return Ok(false);
    }
    if no_link(path)?.len() != entry.size {
        return Ok(false);
    }
    verified(File::open(path).map_err(io_error)?, entry, cancel)
}

fn verified(reader: impl Read, entry: &Entry, cancel: &AtomicBool) -> Result<bool> {
    Ok(match entry.xxh64 {
        Some(hash) => xxh64_reader(reader, cancel)? == hash,
        None => hash_reader(reader, cancel)?.eq_ignore_ascii_case(&entry.sha256),
    })
}

/// Always rehash every existing file, including same-size files. This is both
/// the normal patch check and Verify and repair; there is no stale hash cache.
pub fn repair(
    client: &Path,
    source: &PatchSource,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> Result<PatchReport> {
    cancelled(cancel)?;
    if !valid_hash(&source.manifest_sha256) {
        return Err("Set the trusted manifest SHA-256 pin in Settings.".into());
    }
    let urls: Vec<_> = std::iter::once(source.manifest_url.as_str())
        .chain(source.mirror_url.as_deref().filter(|s| !s.is_empty()))
        .map(Url::parse)
        .collect::<Result<_>>()?;
    let root = root(client)?;
    let status = Progress {
        phase: "Fetching manifest",
        ..Default::default()
    };
    progress(&status);
    let mut parsed = None;
    let mut error = "Could not fetch the patch manifest.".to_owned();
    for attempt in 0..3 {
        for url in &urls {
            cancelled(cancel)?;
            let result = fetch_bytes(url, MANIFEST_LIMIT, cancel)
                .and_then(|bytes| manifest(&bytes, &source.manifest_sha256));
            match result {
                Ok(value) => {
                    parsed = Some(value);
                    break;
                }
                Err(e) => error = e,
            }
        }
        if parsed.is_some() {
            break;
        }
        backoff(attempt, cancel)?;
    }
    let manifest = parsed.ok_or(error)?;
    install(&root, &manifest.files, &urls, cancel, status, progress)
}

/// Verify every file and download the missing or wrong ones from the first URL that serves them.
fn install(
    root: &Path,
    files: &[Entry],
    urls: &[Url],
    cancel: &AtomicBool,
    mut status: Progress,
    mut progress: impl FnMut(&Progress),
) -> Result<PatchReport> {
    let root = root.to_path_buf();
    let mut error = "Could not download a client file.".to_owned();
    // Validate every target before downloading any file, including unsafe links
    // on entries that would otherwise be skipped as up to date.
    for entry in files {
        cancelled(cancel)?;
        target(&root, &entry.path)?;
        target(&root, &format!("{}{PART}", entry.path))?;
    }
    status.files_total = files.len();
    status.bytes_total = files.iter().map(|e| e.size).sum();
    let mut report = PatchReport::default();
    let mut completed_bytes = 0;
    for entry in files {
        cancelled(cancel)?;
        status.current_file = entry.path.clone();
        status.phase = "Verifying";
        status.bytes_done = completed_bytes;
        progress(&status);
        let destination = target(&root, &entry.path)?;
        if !matches(&destination, entry, cancel)? {
            let mut installed = false;
            for attempt in 0..3 {
                for url in urls {
                    cancelled(cancel)?;
                    status.phase = "Downloading";
                    let result = download(
                        &root,
                        entry,
                        &url.file(&entry.path),
                        cancel,
                        |bytes| {
                            status.bytes_done = completed_bytes + bytes;
                            progress(&status);
                        },
                        &mut report.bytes_downloaded,
                    );
                    match result {
                        Ok(()) => {
                            installed = true;
                            break;
                        }
                        Err(e) => error = e,
                    }
                }
                if installed {
                    break;
                }
                backoff(attempt, cancel)?;
            }
            if !installed {
                return Err(error);
            }
            report.files_downloaded += 1;
        }
        completed_bytes += entry.size;
        status.bytes_done = completed_bytes;
        status.files_done += 1;
        progress(&status);
    }
    status.phase = "Ready";
    status.current_file.clear();
    progress(&status);
    Ok(report)
}

pub(crate) fn backoff(attempt: u32, cancel: &AtomicBool) -> Result<()> {
    // (R) Three attempts, 100/200/400 ms backoff; check cancellation every 20 ms.
    for _ in 0..(5 << attempt) {
        cancelled(cancel)?;
        std::thread::sleep(Duration::from_millis(20));
    }
    cancelled(cancel)
}

/// GET `url` (status 200) and read the whole body, at most `limit` bytes. GitHub's API
/// answers chunked with no Content-Length, so the cap is enforced while reading.
pub(crate) fn fetch_bytes(url: &Url, limit: u64, cancel: &AtomicBool) -> Result<Vec<u8>> {
    let mut response = fetch(url, 0)?;
    if response.status != 200 || response.length.is_some_and(|n| n > limit) {
        return Err(format!(
            "The server answered HTTP {} or sent too much data.",
            response.status
        ));
    }
    read_manifest(&mut response, limit, cancel)
}

/// One GET. Only release URLs follow redirects (at most three, each re-validated);
/// every other URL gets the raw response and a 3xx is refused by the caller.
fn fetch(url: &Url, offset: u64) -> Result<native::Response> {
    let mut url = url.clone();
    for _ in 0..4 {
        let response = native::get(&url, offset)?;
        match response.location.as_deref() {
            Some(next) if url.follow && matches!(response.status, 301 | 302 | 303 | 307 | 308) => {
                url = url.redirect(next)?
            }
            _ => return Ok(response),
        }
    }
    Err("Too many redirects.".into())
}

fn read_manifest(
    response: &mut native::Response,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut bytes = Vec::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        cancelled(cancel)?;
        if Instant::now() > deadline {
            return Err("Patch manifest timed out.".into());
        }
        let count = response.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > response.length.unwrap_or(limit).min(limit) {
            return Err("Patch manifest exceeds declared size.".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if response.length.is_some_and(|n| n != bytes.len() as u64) {
        return Err("Incomplete patch manifest.".into());
    }
    Ok(bytes)
}

pub(crate) fn download(
    root: &Path,
    entry: &Entry,
    url: &Url,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
    transferred: &mut u64,
) -> Result<()> {
    let partial_name = format!("{}{PART}", entry.path);
    let partial = target(root, &partial_name)?;
    let mut file = if partial.try_exists().map_err(io_error)? {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&partial)
            .map_err(io_error)?
    } else {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(io_error)?
    };
    let mut offset = file.metadata().map_err(io_error)?.len();
    if offset > entry.size {
        file.set_len(0).map_err(io_error)?;
        offset = 0;
    }
    if offset < entry.size {
        let mut response = fetch(url, offset)?;
        if response.status == 200 {
            // Servers may ignore Range. Accept only the complete expected body.
            if response.length != Some(entry.size) {
                return Err("Patch server reported the wrong file size.".into());
            }
            file.set_len(0).map_err(io_error)?;
            offset = 0;
        } else if response.status != 206
            || offset == 0
            || response.length != Some(entry.size - offset)
            || response.range.as_deref()
                != Some(&format!("bytes {offset}-{}/{}", entry.size - 1, entry.size))
        {
            return Err("Patch server returned an invalid range or size.".into());
        }
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut buffer = [0; 16 * 1024];
        loop {
            cancelled(cancel)?;
            if Instant::now() > deadline {
                return Err("Patch transfer timed out. Retry to resume.".into());
            }
            let count = response.read(&mut buffer).map_err(io_error)?;
            if count == 0 {
                break;
            }
            if offset + count as u64 > entry.size {
                return Err("Patch body exceeds manifest size.".into());
            }
            file.write_all(&buffer[..count]).map_err(io_error)?;
            offset += count as u64;
            *transferred += count as u64;
            progress(offset);
        }
    }
    file.sync_all().map_err(io_error)?;
    if offset != entry.size {
        return Err("Patch download was interrupted. Retry to resume.".into());
    }
    cancelled(cancel)?;
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    if !verified(&mut file, entry, cancel)? {
        // A corrupt prefix cannot be trusted. Keep the temp name but restart
        // from zero on the next bounded attempt; never install these bytes.
        file.set_len(0).map_err(io_error)?;
        let kind = if entry.xxh64.is_some() {
            "XXH64"
        } else {
            "SHA-256"
        };
        return Err(format!(
            "Patch {kind} mismatch. Discarded corrupt partial bytes."
        ));
    }
    drop(file);
    cancelled(cancel)?;
    let destination = target(root, &entry.path)?;
    target(root, &partial_name)?;
    if hard_link(&partial)? {
        return Err("Unsafe patch partial link.".into());
    }
    // Same-directory rename replaces atomically, preserving the old destination
    // on failure. Never remove the installed file first.
    fs::rename(&partial, destination).map_err(io_error)
}

/// L070: the community host's client manifest and file root (the source the community launcher
/// installs from; the owner chose it 2026-10-28). Our releases never carry client files.
pub const CLIENT_MANIFEST_URL: &str = "https://opensourcefreerealms.com/clientmanifest.xml";
const CLIENT_FILES_URL: &str = "https://opensourcefreerealms.com/client/";
/// The client is about 0.9 GB in 1,788 files (2026-10-12 manifest); the caps leave room to grow.
const CLIENT_LIMITS: Limits = Limits {
    files: 20_000,
    file: 1024 * 1024 * 1024,
    total: 16 * 1024 * 1024 * 1024,
};

/// L070: install or repair the whole client in `client` from the community host, resuming
/// partial files. (R) XXH64 catches corruption, not a hostile host: the HTTPS host is trusted,
/// as it is for the executable it serves.
pub fn download_client(
    client: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(&Progress),
) -> Result<PatchReport> {
    cancelled(cancel)?;
    fs::create_dir_all(client).map_err(io_error)?;
    let root = root(client)?;
    let manifest = Url::parse(CLIENT_MANIFEST_URL)?;
    let mut error = "Could not fetch the client manifest.".to_owned();
    let mut files = None;
    for attempt in 0..3 {
        match fetch_bytes(&manifest, MANIFEST_LIMIT, cancel).and_then(|bytes| {
            let text = String::from_utf8(bytes).map_err(|_| "Client manifest is not UTF-8.")?;
            client_manifest(&text)
        }) {
            Ok(parsed) => {
                files = Some(parsed);
                break;
            }
            Err(e) => error = e,
        }
        backoff(attempt, cancel)?;
    }
    let files = files.ok_or(error)?;
    let status = Progress {
        phase: "Verifying",
        ..Default::default()
    };
    install(
        &root,
        &files,
        &[Url::parse(CLIENT_FILES_URL)?],
        cancel,
        status,
        progress,
    )
}

/// L070: `clientmanifest.xml`: nested `<Folder name>` elements holding `<File name size hash>`,
/// the hash an XXH64 in decimal. Only that shape is read; anything else in a tag is refused.
pub(crate) fn client_manifest(xml: &str) -> Result<Vec<Entry>> {
    let bad = || "Invalid client manifest.".to_owned();
    let attribute = |tag: &str, name: &str| {
        let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        tag[start..].split('"').next().map(str::to_owned)
    };
    let (mut folders, mut files) = (Vec::<String>::new(), Vec::new());
    for tag in xml.split('<').skip(1) {
        let tag = tag.split('>').next().ok_or_else(bad)?.trim();
        if tag.starts_with('?') || tag.starts_with("ClientManifest") || tag == "/ClientManifest" {
            continue;
        }
        if tag == "/Folder" {
            folders.pop().ok_or_else(bad)?;
        } else if let Some(rest) = tag.strip_prefix("Folder") {
            if !rest.ends_with('/') {
                folders.push(attribute(tag, "name").unwrap_or_default());
            }
        } else if tag.starts_with("File ") && tag.ends_with('/') {
            let name = attribute(tag, "name").ok_or_else(bad)?;
            let number = |key| attribute(tag, key).and_then(|v| v.parse::<u64>().ok());
            let (Some(size), Some(hash)) = (number("size"), number("hash")) else {
                return Err(bad());
            };
            let mut path: Vec<&str> = folders
                .iter()
                .map(String::as_str)
                .filter(|f| !f.is_empty())
                .collect();
            path.push(&name);
            files.push(Entry {
                path: path.join("/"),
                size,
                sha256: String::new(),
                asset: None,
                xxh64: Some(hash),
            });
        } else {
            return Err(bad());
        }
    }
    if !folders.is_empty() {
        return Err(bad());
    }
    check(&files, &CLIENT_LIMITS)?;
    Ok(files)
}

/// XXH64 with seed 0 over a whole reader (client files are tens of MB at most).
fn xxh64_reader(mut reader: impl Read, cancel: &AtomicBool) -> Result<u64> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        cancelled(cancel)?;
        let count = reader.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            return Ok(xxh64(&bytes));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

/// XXH64, seed 0 (the xxHash specification); checked against the standard vectors and against
/// real client files' manifest hashes.
pub fn xxh64(data: &[u8]) -> u64 {
    const P1: u64 = 0x9E37_79B1_85EB_CA87;
    const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
    const P3: u64 = 0x1656_67B1_9E37_79F9;
    const P4: u64 = 0x85EB_CA77_C2B2_AE63;
    const P5: u64 = 0x27D4_EB2F_1656_67C5;
    let round = |acc: u64, input: u64| {
        acc.wrapping_add(input.wrapping_mul(P2))
            .rotate_left(31)
            .wrapping_mul(P1)
    };
    let word = |at: &[u8]| u64::from_le_bytes(at[..8].try_into().unwrap());
    let mut rest = data;
    let mut hash = if data.len() >= 32 {
        let mut v = [P1.wrapping_add(P2), P2, 0, P1.wrapping_neg()];
        while rest.len() >= 32 {
            for (lane, value) in v.iter_mut().enumerate() {
                *value = round(*value, word(&rest[lane * 8..]));
            }
            rest = &rest[32..];
        }
        let mut hash = v[0]
            .rotate_left(1)
            .wrapping_add(v[1].rotate_left(7))
            .wrapping_add(v[2].rotate_left(12))
            .wrapping_add(v[3].rotate_left(18));
        for value in v {
            hash = (hash ^ round(0, value)).wrapping_mul(P1).wrapping_add(P4);
        }
        hash
    } else {
        P5
    };
    hash = hash.wrapping_add(data.len() as u64);
    while rest.len() >= 8 {
        hash ^= round(0, word(rest));
        hash = hash.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        rest = &rest[8..];
    }
    if rest.len() >= 4 {
        let half = u32::from_le_bytes(rest[..4].try_into().unwrap());
        hash ^= u64::from(half).wrapping_mul(P1);
        hash = hash.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
        rest = &rest[4..];
    }
    for &byte in rest {
        hash ^= u64::from(byte).wrapping_mul(P5);
        hash = hash.rotate_left(11).wrapping_mul(P1);
    }
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(P2);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(P3);
    hash ^ (hash >> 32)
}

/// Hosts a release download may touch, over HTTPS port 443 only.
const GITHUB_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

#[derive(Clone)]
pub(crate) struct Url {
    host: String,
    port: u16,
    path: String,
    tls: bool,
    /// Release URLs only: follow redirects that stay on [`GITHUB_HOSTS`].
    follow: bool,
}
impl Url {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let invalid =
            || "Patch URL must be HTTPS (HTTP is allowed only for literal loopback).".to_owned();
        if text.len() > 2048
            || !text.is_ascii()
            || text.bytes().any(|b| b <= 32 || b == 127)
            || text.contains(['@', '\\', '?', '#'])
        {
            return Err(invalid());
        }
        let (tls, rest) = if let Some(s) = text.strip_prefix("https://") {
            (true, s)
        } else if let Some(s) = text.strip_prefix("http://") {
            (false, s)
        } else {
            return Err(invalid());
        };
        let (authority, path) = rest.split_once('/').ok_or_else(invalid)?;
        let (host, port) = if authority.starts_with('[') {
            let end = authority.find(']').ok_or_else(invalid)?;
            let port = &authority[end + 1..];
            (
                &authority[1..end],
                if port.is_empty() {
                    if tls { 443 } else { 80 }
                } else {
                    port.strip_prefix(':')
                        .ok_or_else(invalid)?
                        .parse::<u16>()
                        .map_err(|_| invalid())?
                },
            )
        } else if let Some((host, port)) = authority.split_once(':') {
            (host, port.parse::<u16>().map_err(|_| invalid())?)
        } else {
            (authority, if tls { 443 } else { 80 })
        };
        if host.is_empty()
            || port == 0
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
            || (!tls
                && !host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback()))
        {
            return Err(invalid());
        }
        Ok(Self {
            host: host.into(),
            port,
            path: format!("/{path}"),
            tls,
            follow: false,
        })
    }
    /// A release URL: HTTPS on a GitHub host, or literal-loopback HTTP (the test hosts).
    pub(crate) fn release(text: &str) -> Result<Self> {
        let mut url = Self::parse(text)?;
        url.follow = true;
        if url.tls
            && !(url.port == 443
                && GITHUB_HOSTS
                    .iter()
                    .any(|h| url.host.eq_ignore_ascii_case(h)))
        {
            return Err("Release downloads are limited to GitHub over HTTPS.".into());
        }
        Ok(url)
    }
    /// A GitHub API list page: the only query is `per_page`, a number from code, never from input.
    pub(crate) fn release_list(text: &str, per_page: u8) -> Result<Self> {
        let mut url = Self::release(text)?;
        url.path = format!("{}?per_page={per_page}", url.path);
        Ok(url)
    }
    /// The next hop of a redirect: absolute, same rules as [`Self::release`], and a
    /// plain-HTTP loopback hop is possible only from a loopback origin. Signed CDN
    /// links carry a query string, so it is checked separately and kept verbatim.
    fn redirect(&self, location: &str) -> Result<Self> {
        let refused = || "Refused an unsafe release redirect.".to_owned();
        let (base, query) = location.split_once('?').unwrap_or((location, ""));
        if location.len() > 4096
            || !query
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~%=&+:/,".contains(&b))
        {
            return Err(refused());
        }
        let mut next = Self::release(base).map_err(|_| refused())?;
        if self.tls && !next.tls {
            return Err(refused());
        }
        if !query.is_empty() {
            next.path = format!("{}?{query}", next.path);
        }
        Ok(next)
    }
    fn file(&self, path: &str) -> Self {
        let mut url = self.clone();
        let base = self
            .path
            .rsplit_once('/')
            .map(|(base, _)| base)
            .unwrap_or("");
        url.path = format!(
            "{base}/{}",
            path.replace(' ', "%20")
                .replace('(', "%28")
                .replace(')', "%29")
        );
        url
    }
}

/// OS SHA-256, including standard vectors in the integration tests.
pub fn sha256(bytes: &[u8]) -> Result<String> {
    hash_reader(bytes, &AtomicBool::new(false))
}

/// OS MD5 as upper-case hex, the form a locale `.dir` header records (not a security check).
pub fn md5_upper(bytes: &[u8]) -> Result<String> {
    let mut hash = native::Hash::open("MD5")?;
    for chunk in bytes.chunks(64 * 1024) {
        hash.update(chunk)?;
    }
    let mut digest = [0; 16];
    hash.finish_to(&mut digest)?;
    Ok(digest.iter().map(|b| format!("{b:02X}")).collect())
}
fn hash_reader(mut reader: impl Read, cancel: &AtomicBool) -> Result<String> {
    let mut hash = native::Hash::new()?;
    let mut buffer = [0; 64 * 1024];
    loop {
        cancelled(cancel)?;
        let count = reader.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count])?;
    }
    Ok(hash.finish()?.iter().map(|b| format!("{b:02x}")).collect())
}

// Native Windows services are already installed. No crate, hand-rolled TLS,
// crypto primitive, shell helper or global tool is needed.
#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::c_void,
        io,
        ptr::{null, null_mut},
    };
    type Handle = *mut c_void;
    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(
            agent: *const u16,
            access: u32,
            proxy: *const u16,
            bypass: *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpSetTimeouts(
            handle: Handle,
            resolve: i32,
            connect: i32,
            send: i32,
            receive: i32,
        ) -> i32;
        fn WinHttpSetOption(handle: Handle, option: u32, buffer: *const c_void, length: u32)
        -> i32;
        fn WinHttpConnect(handle: Handle, server: *const u16, port: u16, reserved: u32) -> Handle;
        fn WinHttpOpenRequest(
            handle: Handle,
            verb: *const u16,
            object: *const u16,
            version: *const u16,
            referer: *const u16,
            types: *const *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpSendRequest(
            handle: Handle,
            headers: *const u16,
            length: u32,
            optional: *const c_void,
            optional_length: u32,
            total: u32,
            context: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(handle: Handle, reserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(
            handle: Handle,
            level: u32,
            name: *const u16,
            buffer: *mut c_void,
            length: *mut u32,
            index: *mut u32,
        ) -> i32;
        fn WinHttpReadData(handle: Handle, buffer: *mut c_void, length: u32, read: *mut u32)
        -> i32;
        fn WinHttpCloseHandle(handle: Handle) -> i32;
    }
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptOpenAlgorithmProvider(
            handle: *mut Handle,
            name: *const u16,
            provider: *const u16,
            flags: u32,
        ) -> i32;
        fn BCryptCloseAlgorithmProvider(handle: Handle, flags: u32) -> i32;
        fn BCryptCreateHash(
            algorithm: Handle,
            hash: *mut Handle,
            object: *mut u8,
            object_size: u32,
            secret: *const u8,
            secret_size: u32,
            flags: u32,
        ) -> i32;
        fn BCryptHashData(hash: Handle, input: *const u8, length: u32, flags: u32) -> i32;
        fn BCryptFinishHash(hash: Handle, output: *mut u8, length: u32, flags: u32) -> i32;
        fn BCryptDestroyHash(hash: Handle) -> i32;
    }
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }
    struct Internet(Handle);
    impl Internet {
        fn new(handle: Handle) -> Result<Self> {
            if handle.is_null() {
                Err("Patch connection failed. Check the URL and network.".into())
            } else {
                Ok(Self(handle))
            }
        }
    }
    impl Drop for Internet {
        fn drop(&mut self) {
            // SAFETY: this wrapper uniquely owns the live WinHTTP handle.
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
    pub(super) struct Response {
        request: Internet,
        _connection: Internet,
        _session: Internet,
        pub status: u32,
        /// `None` when the server sent no Content-Length (chunked); downloads refuse that.
        pub length: Option<u64>,
        pub range: Option<String>,
        pub location: Option<String>,
    }
    fn header(request: Handle, kind: u32) -> Result<Option<String>> {
        let mut buffer = [0_u16; 4096];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        // SAFETY: buffer and length are valid writable allocations. Null name
        // and index select the first standard header identified by kind.
        if unsafe {
            WinHttpQueryHeaders(
                request,
                kind,
                null(),
                buffer.as_mut_ptr().cast(),
                &mut size,
                null_mut(),
            )
        } == 0
        {
            if io::Error::last_os_error().raw_os_error() == Some(12150) {
                return Ok(None);
            }
            return Err("Patch response headers are invalid or too large.".into());
        }
        if size as usize > std::mem::size_of_val(&buffer) || size % 2 != 0 {
            return Err("Invalid patch response header.".into());
        }
        Ok(Some(
            String::from_utf16_lossy(&buffer[..size as usize / 2])
                .trim_end_matches('\0')
                .into(),
        ))
    }
    pub(super) fn get(url: &Url, offset: u64) -> Result<Response> {
        // SAFETY: all UTF-16 inputs are NUL terminated and live through each
        // synchronous call. Handles are owned in dependency order by Response.
        // No certificate validation flags are disabled; TLS belongs to WinHTTP.
        unsafe {
            let session = Internet::new(WinHttpOpen(
                wide("FreeRealmsExpanded/patch-v1").as_ptr(),
                1,
                null(),
                null(),
                0,
            ))?;
            if WinHttpSetTimeouts(session.0, 3000, 3000, 3000, 3000) == 0 {
                return Err("Could not set patch timeouts.".into());
            }
            // RECEIVE_RESPONSE_TIMEOUT bounds all headers, even when a peer
            // trickles bytes inside the ordinary per-read timeout. Cap total
            // headers too, before WinHTTP buffers the response.
            for (option, value) in [(7, 3000_u32), (91, 8192_u32)] {
                if WinHttpSetOption(session.0, option, (&value as *const u32).cast(), 4) == 0 {
                    return Err("Could not bound patch response headers.".into());
                }
            }
            // Explicit TLS 1.2 minimum. TLS 1.3 can be enabled after fresh-PC acceptance.
            let tls12 = 0x800_u32;
            if WinHttpSetOption(session.0, 84, (&tls12 as *const u32).cast(), 4) == 0 {
                return Err("Could not configure patch TLS.".into());
            }
            let connection = Internet::new(WinHttpConnect(
                session.0,
                wide(&url.host).as_ptr(),
                url.port,
                0,
            ))?;
            let request = Internet::new(WinHttpOpenRequest(
                connection.0,
                wide("GET").as_ptr(),
                wide(&url.path).as_ptr(),
                wide("HTTP/1.1").as_ptr(),
                null(),
                null(),
                if url.tls { 0x800000 } else { 0 },
            ))?;
            // Disable redirects, cookies and automatic credential negotiation.
            let disabled = 7_u32;
            if WinHttpSetOption(request.0, 63, (&disabled as *const u32).cast(), 4) == 0 {
                return Err("Could not secure patch request.".into());
            }
            let range = if offset > 0 {
                format!("Range: bytes={offset}-\r\n")
            } else {
                String::new()
            };
            let headers = wide(&format!("Accept-Encoding: identity\r\n{range}"));
            if WinHttpSendRequest(
                request.0,
                headers.as_ptr(),
                (headers.len() - 1) as u32,
                null(),
                0,
                0,
                0,
            ) == 0
                || WinHttpReceiveResponse(request.0, null_mut()) == 0
            {
                return Err("Patch transfer failed. Check the network or mirror.".into());
            }
            let status = header(request.0, 19)?
                .ok_or("Missing patch HTTP status.")?
                .parse()
                .map_err(|_| "Invalid patch HTTP status.")?;
            let location = header(request.0, 33)?;
            let length = header(request.0, 5)?
                .map(|text| text.parse::<u64>())
                .transpose()
                .map_err(|_| "Invalid patch Content-Length.")?;
            if header(request.0, 29)?.is_some_and(|s| !s.eq_ignore_ascii_case("identity")) {
                return Err("Compressed patch responses are not accepted.".into());
            }
            let range = header(request.0, 53)?;
            Ok(Response {
                request,
                _connection: connection,
                _session: session,
                status,
                length,
                range,
                location,
            })
        }
    }
    impl Read for Response {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let mut count = 0;
            let length = buffer.len().min(u32::MAX as usize) as u32;
            // SAFETY: buffer is writable for length; count is a live DWORD.
            if unsafe {
                WinHttpReadData(
                    self.request.0,
                    buffer.as_mut_ptr().cast(),
                    length,
                    &mut count,
                )
            } == 0
            {
                Err(io::Error::last_os_error())
            } else {
                Ok(count as usize)
            }
        }
    }
    pub(super) struct Hash {
        hash: Handle,
        algorithm: Handle,
    }
    impl Hash {
        pub fn new() -> Result<Self> {
            Self::open("SHA256")
        }
        /// L073: the locale `.dir` header carries an MD5 of the `.dat`.
        pub fn open(name: &str) -> Result<Self> {
            let mut value = Self {
                hash: null_mut(),
                algorithm: null_mut(),
            };
            // SAFETY: output handles are writable. Windows allocates the hash
            // object when its buffer is null (Windows 7+); Drop frees both.
            unsafe {
                if BCryptOpenAlgorithmProvider(&mut value.algorithm, wide(name).as_ptr(), null(), 0)
                    < 0
                    || BCryptCreateHash(
                        value.algorithm,
                        &mut value.hash,
                        null_mut(),
                        0,
                        null(),
                        0,
                        0,
                    ) < 0
                {
                    return Err(format!("Windows {name} service unavailable.").into());
                }
            }
            Ok(value)
        }
        pub fn update(&mut self, bytes: &[u8]) -> Result<()> {
            // SAFETY: live hash and input allocation; callers use <=64 KiB.
            if unsafe { BCryptHashData(self.hash, bytes.as_ptr(), bytes.len() as u32, 0) } < 0 {
                return Err("SHA-256 failed.".into());
            }
            Ok(())
        }
        pub fn finish(&mut self) -> Result<[u8; 32]> {
            let mut digest = [0; 32];
            self.finish_to(&mut digest)?;
            Ok(digest)
        }
        /// `digest` must be exactly the algorithm's output size (32 SHA256, 16 MD5).
        pub fn finish_to(&mut self, digest: &mut [u8]) -> Result<()> {
            // SAFETY: the caller sizes digest to the algorithm's output; hash is live.
            let length = digest.len() as u32;
            if unsafe { BCryptFinishHash(self.hash, digest.as_mut_ptr(), length, 0) } < 0 {
                return Err("Hash failed.".into());
            }
            Ok(())
        }
    }
    impl Drop for Hash {
        fn drop(&mut self) {
            // SAFETY: each non-null handle is owned uniquely, hash before provider.
            unsafe {
                if !self.hash.is_null() {
                    BCryptDestroyHash(self.hash);
                }
                if !self.algorithm.is_null() {
                    BCryptCloseAlgorithmProvider(self.algorithm, 0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Url, client_manifest, xxh64};

    /// L070: the xxHash specification's vectors (seed 0); real client files were also checked
    /// against their manifest hashes (actionIndex.dat, FreeRealms.exe) outside the repository.
    #[test]
    fn xxh64_matches_the_reference_vectors() {
        assert_eq!(xxh64(b""), 0xEF46_DB37_51D8_E999);
        assert_eq!(xxh64(b"abc"), 0x44BC_2CF5_AD77_0999);
        // Long input exercises the four-lane stripe loop and the 8/4/1-byte tails.
        let long: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        assert_ne!(xxh64(&long), xxh64(&long[1..]));
    }

    /// L070: the live community manifest parses whole (1,788 files on 2026-10-12) and lists the
    /// executable and the DirectX DLL beside it.
    #[test]
    #[ignore = "needs network"]
    fn live_client_manifest_parses() {
        let url = Url::parse(super::CLIENT_MANIFEST_URL).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let bytes = super::fetch_bytes(&url, super::MANIFEST_LIMIT, &cancel).unwrap();
        let files = client_manifest(std::str::from_utf8(&bytes).unwrap()).unwrap();
        assert!(files.len() > 1000, "{}", files.len());
        for name in ["FreeRealms.exe", "d3dx9_31.dll"] {
            assert!(files.iter().any(|f| f.path == name), "{name}");
        }
    }

    /// L070: nested folders make the paths; unsafe names, broken nesting and stray tags refuse
    /// the whole manifest.
    #[test]
    fn the_client_manifest_reads_nested_folders_and_refuses_unsafe_entries() {
        let xml = r#"<?xml version='1.0' encoding='utf-8'?>
<ClientManifest version="1" languages="en_us">
  <Folder>
    <Folder name="data" />
    <Folder name="tcg">
      <File name="index.dat" size="15237" hash="16582454773280641639" />
    </Folder>
    <File name="FreeRealms.exe" size="25067520" hash="14615788379343038154" />
  </Folder>
</ClientManifest>"#;
        let files = client_manifest(xml).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["tcg/index.dat", "FreeRealms.exe"]);
        assert_eq!(files[0].xxh64, Some(16582454773280641639));
        assert_eq!(files[1].size, 25067520);
        for bad in [
            xml.replace("index.dat", "../escape.dat"),
            xml.replace("index.dat", "CON.dat"),
            xml.replace(
                "<Folder name=\"tcg\">",
                "<Folder name=\"tcg\"><Folder name=\"x\">",
            ),
            xml.replace("hash=\"16582454773280641639\"", "hash=\"abc\""),
            xml.replace("<Folder name=\"data\" />", "<Script name=\"x\" />"),
        ] {
            assert!(client_manifest(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn release_lists_add_only_a_page_size_to_a_github_url() {
        let url = Url::release_list("https://api.github.com/repos/a/b/releases", 3).unwrap();
        assert_eq!(url.path, "/repos/a/b/releases?per_page=3");
        assert!(Url::release_list("https://example.com/releases", 3).is_err());
    }

    #[test]
    fn release_urls_are_github_https_or_loopback_http() {
        for ok in [
            "https://api.github.com/repos/a/b/releases/latest",
            "https://github.com/a/b/releases/download/v1/x",
            "http://127.0.0.1:8080/x",
            "http://[::1]:1/x",
        ] {
            assert!(Url::release(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://github.com/x",
            "http://example.com/x",
            "https://example.com/x",
            "https://github.com.evil.example/x",
            "https://github.com:8443/x",
            "https://github.com@evil.example/x",
            "https://api.github.com\\@evil/x",
            "ftp://github.com/x",
            "https://objects.githubusercontent.com.evil/x",
            "https://github.com/x?a=b",
        ] {
            assert!(Url::release(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn redirects_stay_on_github_and_never_downgrade() {
        let github = Url::release("https://github.com/a/b/releases/download/v1/x").unwrap();
        for ok in [
            "https://release-assets.githubusercontent.com/p/1?sp=r&sig=ab%2Fc%3D&se=2026-10-26T22%3A00%3A00Z",
            "https://objects.githubusercontent.com/p",
        ] {
            assert!(github.redirect(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://127.0.0.1/x",
            "http://github.com/x",
            "https://evil.example/x",
            "/relative/path",
            "https://github.com/x?a=<script>",
            "https://github.com/x?a b",
            "https://github.com/x#frag",
        ] {
            assert!(github.redirect(bad).is_err(), "{bad}");
        }
        let local = Url::release("http://127.0.0.1:9/x").unwrap();
        assert!(local.redirect("http://127.0.0.1:9/y?s=1").is_ok());
        assert!(local.redirect("http://example.com/y").is_err());
    }
}
