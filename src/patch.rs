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
    if value.version != 1 || value.files.is_empty() || value.files.len() > limits.files {
        return Err("Unsupported or empty patch manifest.".into());
    }
    let mut names = BTreeSet::new();
    let mut total = 0_u64;
    for entry in &value.files {
        if !valid_path(&entry.path) || !valid_hash(&entry.sha256) || entry.size > limits.file {
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
    Ok(value)
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
    Ok(hash_reader(File::open(path).map_err(io_error)?, cancel)?
        .eq_ignore_ascii_case(&entry.sha256))
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
    let mut status = Progress {
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
    let manifest = parsed.ok_or_else(|| error.clone())?;
    // Validate every target before downloading any file, including unsafe links
    // on entries that would otherwise be skipped as up to date.
    for entry in &manifest.files {
        cancelled(cancel)?;
        target(&root, &entry.path)?;
        target(&root, &format!("{}{PART}", entry.path))?;
    }
    status.files_total = manifest.files.len();
    status.bytes_total = manifest.files.iter().map(|e| e.size).sum();
    let mut report = PatchReport::default();
    let mut completed_bytes = 0;
    for entry in &manifest.files {
        cancelled(cancel)?;
        status.current_file = entry.path.clone();
        status.phase = "Verifying";
        status.bytes_done = completed_bytes;
        progress(&status);
        let destination = target(&root, &entry.path)?;
        if !matches(&destination, entry, cancel)? {
            let mut installed = false;
            for attempt in 0..3 {
                for url in &urls {
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
    if !hash_reader(&mut file, cancel)?.eq_ignore_ascii_case(&entry.sha256) {
        // A corrupt prefix cannot be trusted. Keep the temp name but restart
        // from zero on the next bounded attempt; never install these bytes.
        file.set_len(0).map_err(io_error)?;
        return Err("Patch SHA-256 mismatch. Discarded corrupt partial bytes.".into());
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
            let mut value = Self {
                hash: null_mut(),
                algorithm: null_mut(),
            };
            // SAFETY: output handles are writable. Windows allocates the hash
            // object when its buffer is null (Windows 7+); Drop frees both.
            unsafe {
                if BCryptOpenAlgorithmProvider(
                    &mut value.algorithm,
                    wide("SHA256").as_ptr(),
                    null(),
                    0,
                ) < 0
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
                    return Err("Windows SHA-256 service unavailable.".into());
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
            // SAFETY: SHA256's output is exactly 32 bytes; hash is live.
            if unsafe { BCryptFinishHash(self.hash, digest.as_mut_ptr(), 32, 0) } < 0 {
                return Err("SHA-256 failed.".into());
            }
            Ok(digest)
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
    use super::Url;

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
