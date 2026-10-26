//! Local client preparation and per-server key-pair authentication.

pub mod assets;
pub mod auth;
pub mod patch;

/// Handle enrolment commands before opening the UI. `None` leaves ordinary UI
/// arguments to the caller. The only successful output is a public key (hex).
/// The binary's entry-point dispatch is wired by the lead, outside W100's scope.
pub fn key_command<I, S>(args: I) -> Option<Result<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let command = args.next()?;
    let command = command.as_ref();
    if command != "--new-key" && command != "--print-public-key" {
        return None;
    }
    let profile = match args.next() {
        Some(profile) => profile,
        None => return Some(Err("Key commands require exactly one profile ID.".into())),
    };
    if args.next().is_some() {
        return Some(Err("Key commands require exactly one profile ID.".into()));
    }
    Some(
        match command {
            "--new-key" => auth::new_key(profile.as_ref()),
            _ => auth::print_public_key(profile.as_ref()),
        }
        .map_err(|error| error.to_string()),
    )
}

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::{IpAddr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, String>;
const LOCALE: [&str; 2] = ["en_us_data.dat", "en_us_data.dir"];
const CONFIG_LIMIT: u64 = 1024 * 1024;
const LOCALE_LIMIT: u64 = 64 * 1024 * 1024;

/// (R) Launcher preferences; preserve the Python launcher's windowed defaults.
#[derive(Clone, Copy)]
pub struct LaunchSettings {
    pub width: u32,
    pub height: u32,
}

impl Default for LaunchSettings {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 768,
        }
    }
}

impl LaunchSettings {
    pub fn validate(self) -> Result<()> {
        // (R) Bound accidental/pathological local settings, not recovered game limits.
        if !(640..=3840).contains(&self.width) || !(480..=2160).contains(&self.height) {
            return Err("Window size must be 640–3840 by 480–2160.".into());
        }
        Ok(())
    }
}

/// W83 implements this boundary: perform per-server key authentication privately
/// and return only its short-lived Session. Never return/log the private key.
pub trait LoginProvider {
    fn session(&mut self, endpoint: &str) -> Result<SessionToken>;
}

/// Intentionally has no Debug/Display implementation: never print bearer values.
pub struct SessionToken(String);

impl SessionToken {
    pub fn new(value: String) -> Result<Self> {
        if value.is_empty()
            || value.len() > 1024
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("Login returned an invalid session token.".into());
        }
        Ok(Self(value))
    }
}

/// (R) Explicit local compatibility stub, not authentication. No DNS resolution,
/// enrolment, passwords, private keys, remote access or silent auth fallback.
pub struct LocalLoginStub;

impl LoginProvider for LocalLoginStub {
    fn session(&mut self, endpoint: &str) -> Result<SessionToken> {
        validate_endpoint(endpoint)?;
        let address: SocketAddr = endpoint.parse().map_err(|_| {
            "Login stub permits literal loopback addresses only; remote login awaits W83."
                .to_owned()
        })?;
        if !address.ip().is_loopback() {
            return Err("Remote login is unavailable in this skeleton (W83).".into());
        }
        SessionToken::new("local-session".into())
    }
}

pub fn validate_endpoint(value: &str) -> Result<()> {
    let bad = || "Use host:port, with IPv6 in brackets; port must be 1–65535.".to_owned();
    if value.len() > 261 {
        return Err(bad());
    }
    let port = if let Some(rest) = value.strip_prefix('[') {
        let (host, port) = rest.split_once("]:").ok_or_else(bad)?;
        host.parse::<Ipv6Addr>().map_err(|_| bad())?;
        port
    } else {
        let (host, port) = value.rsplit_once(':').ok_or_else(bad)?;
        if host.parse::<IpAddr>().is_err() {
            if host.is_empty()
                || host.len() > 253
                || !host.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label.as_bytes()[0].is_ascii_alphanumeric()
                        && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                return Err(bad());
            }
        } else if host.contains(':') {
            return Err(bad());
        }
        port
    };
    if port.is_empty()
        || !port.bytes().all(|b| b.is_ascii_digit())
        || port.parse::<u16>().ok().filter(|p| *p != 0).is_none()
    {
        return Err(bad());
    }
    Ok(())
}

fn io_error(_: io::Error) -> String {
    // Local paths and auth-bearing batch/config contents do not enter UI logs.
    "Client file operation failed. Check folder permissions and available disk space.".into()
}

fn no_link(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        linked || metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT, includes junctions.
    };
    if linked {
        return Err("Symlinks and reparse points are not allowed inside the client folder.".into());
    }
    Ok(metadata)
}

fn regular(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = root.join(relative);
    if !no_link(&path)?.is_file() || !path.canonicalize().map_err(io_error)?.starts_with(root) {
        return Err(format!("Missing or unsafe client file: {relative}"));
    }
    Ok(path)
}

fn hard_link(path: &Path) -> Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // BY_HANDLE_FILE_INFORMATION is thirteen DWORDs (the three FILETIMEs
        // each occupy two DWORDs); nNumberOfLinks is DWORD 10. Bind this one
        // existing OS function without adding a platform wrapper dependency.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                info: *mut [u32; 13],
            ) -> i32;
        }
        let file = File::open(path).map_err(io_error)?;
        let mut info = [0_u32; 13];
        // SAFETY: file owns a live handle. info has the Win32 structure's exact
        // 52-byte size and DWORD alignment and remains valid through this call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io_error(io::Error::last_os_error()));
        }
        Ok(info[10] > 1)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(fs::metadata(path).map_err(io_error)?.nlink() > 1)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        Err("Unsupported file safety checks on this platform.".into())
    }
}

fn safe_target(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = root.join(relative);
    if path.try_exists().map_err(io_error)? {
        regular(root, relative)?;
        if hard_link(&path)? {
            return Err("Refusing to replace a hard-linked client file.".into());
        }
    } else if fs::symlink_metadata(&path).is_ok() {
        return Err("Refusing a dangling link at a client target.".into());
    }
    if !path
        .parent()
        .ok_or("Missing target parent")?
        .canonicalize()
        .map_err(io_error)?
        .starts_with(root)
    {
        return Err("Client target escapes its folder.".into());
    }
    Ok(path)
}

fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io_error)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > limit {
        return Err("Client configuration or locale file exceeds its size limit.".into());
    }
    Ok(bytes)
}

fn text(root: &Path, name: &str) -> Result<String> {
    let bytes = bounded_read(&regular(root, name)?, CONFIG_LIMIT)?;
    let text =
        String::from_utf8(bytes).map_err(|_| "Client configuration must be UTF-8.".to_owned())?;
    Ok(text.trim_start_matches('\u{feff}').replace("\r\n", "\n"))
}

fn replace_setting(text: &str, key: &str, value: &str) -> Result<String> {
    let prefix = format!("{key}=");
    if text
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .count()
        != 1
    {
        return Err(format!("Configuration must have exactly one {key}= line."));
    }
    Ok(text
        .split('\n')
        .map(|line| {
            if line.starts_with(&prefix) {
                format!("{prefix}{value}")
            } else {
                line.into()
            }
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn offline_urls(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest
        .find("http://")
        .into_iter()
        .chain(rest.find("https://"))
        .min()
    {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        output.push_str("http://127.0.0.1:9/");
        rest = &rest[end..];
    }
    output.push_str(rest);
    output
        .split('\n')
        .map(|line| {
            if line.starts_with("Address=lvsadp") {
                "Address=127.0.0.1"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Parse data only. Never execute the batch, expand variables or invoke a shell.
fn tokens(line: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            '&' | '|' | '<' | '>' | '^' | '%' | '\0' => {
                return Err("Unsupported batch syntax in client arguments.".into());
            }
            _ => word.push(c),
        }
    }
    if quoted {
        return Err("Unclosed quote in client arguments.".into());
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

fn launch_arguments(batch: &str, endpoint: &str, token: SessionToken) -> Result<Vec<String>> {
    let mut commands = Vec::new();
    for line in batch.lines() {
        let trimmed = line.trim_start().trim_start_matches('@');
        // Only tokenize candidate executable commands, not unrelated echo text.
        let first = trimmed.split_whitespace().next().unwrap_or("");
        if !first.eq_ignore_ascii_case("start") && !first.eq_ignore_ascii_case("FreeRealms.exe") {
            continue;
        }
        let words = tokens(trimmed)?;
        let mut index = usize::from(
            words
                .first()
                .is_some_and(|s| s.eq_ignore_ascii_case("start")),
        );
        if index == 1 {
            while words.get(index).is_some_and(|s| s.starts_with('/')) {
                index += 1;
            }
        }
        if words
            .get(index)
            .is_some_and(|s| s.eq_ignore_ascii_case("FreeRealms.exe"))
        {
            commands.push(words[index + 1..].to_vec());
        }
    }
    if commands.len() != 1 {
        return Err("FreeRealms.bat must contain exactly one FreeRealms.exe command.".into());
    }
    let mut arguments = Vec::new();
    let mut ini = 0;
    for argument in &commands[0] {
        let (name, _) = argument
            .split_once('=')
            .ok_or("Client arguments must be name=value pairs.")?;
        match name.to_ascii_lowercase().as_str() {
            "guid" | "ticket" | "key" | "cassessionid" | "sessionid" | "server" => (),
            "inifile" => {
                ini += 1;
                arguments.push("inifile=Baseline.ini".into());
            }
            "internationalization:locale" => (), // Append exactly one validated override.
            "showmemberloadingscreen" | "country" => arguments.push(argument.clone()),
            _ => return Err("Unsupported argument name in FreeRealms.bat.".into()),
        }
    }
    if ini != 1 {
        return Err("FreeRealms.bat must have exactly one inifile= argument.".into());
    }
    arguments.extend([
        format!("Server={endpoint}"),
        format!("SessionId={}", token.0),
        "Internationalization:Locale=8".into(),
        "GameAccountManagement:UriRoot=http://127.0.0.1:9/".into(),
    ]);
    Ok(arguments)
}

struct Change {
    relative: String,
    contents: Vec<u8>,
    original: Option<Vec<u8>>,
}

/// No Debug: arguments contain the short-lived Session handoff.
pub struct LaunchPlan {
    root: PathBuf,
    changes: Vec<Change>,
    arguments: Vec<String>,
    backup: PathBuf,
    assets: assets::Responder,
}

impl LaunchPlan {
    pub fn build(
        client: &Path,
        endpoint: &str,
        locale: Option<&Path>,
        settings: LaunchSettings,
        login: &mut dyn LoginProvider,
    ) -> Result<Self> {
        validate_endpoint(endpoint)?;
        settings.validate()?;
        let root = client.canonicalize().map_err(io_error)?;
        if !root.is_dir() || !no_link(&root.join("locale"))?.is_dir() {
            return Err("Choose a client folder containing a regular locale directory.".into());
        }
        let mut signature = [0; 2];
        File::open(regular(&root, "FreeRealms.exe")?)
            .map_err(io_error)?
            .read_exact(&mut signature)
            .map_err(io_error)?;
        if signature != *b"MZ" {
            return Err("FreeRealms.exe has no Windows executable signature.".into());
        }
        for name in LOCALE {
            regular(&root, &format!("locale/{name}"))?;
        }
        let assets = assets::Responder::start(&root)?;
        let baseline = replace_setting(
            &offline_urls(&replace_setting(
                &text(&root, "ClientConfig.ini")?,
                "Server",
                endpoint,
            )?),
            "IndirectServerAddress",
            &assets.url,
        )?;
        let mut options = text(&root, "UserOptions.ini")?;
        for (key, value) in [
            ("WindowedWidth", settings.width.to_string()),
            ("WindowedHeight", settings.height.to_string()),
            ("Mode", "Windowed".into()),
            ("Maximized", "0".into()),
            ("Master", "0.000000".into()),
        ] {
            options = replace_setting(&options, key, &value)?;
        }
        let mut contents = vec![
            ("Baseline.ini".into(), baseline.into_bytes()),
            ("UserOptions.ini".into(), options.into_bytes()),
        ];
        if let Some(locale) = locale {
            let source = locale.canonicalize().map_err(io_error)?;
            if !source.is_dir() {
                return Err("Locale overlay must be a directory.".into());
            }
            for name in LOCALE {
                let from = regular(&source, name)?;
                let to = safe_target(&root, &format!("locale/{name}"))?;
                if from.canonicalize().map_err(io_error)? != to.canonicalize().map_err(io_error)? {
                    contents.push((format!("locale/{name}"), bounded_read(&from, LOCALE_LIMIT)?));
                }
            }
        }
        let mut changes = Vec::new();
        for (relative, bytes) in contents {
            let target = safe_target(&root, &relative)?;
            let original = if target.exists() {
                Some(bounded_read(&target, LOCALE_LIMIT)?)
            } else {
                None
            };
            changes.push(Change {
                relative,
                contents: bytes,
                original,
            });
        }
        let parent = root.join(".friend-launcher-backups");
        if parent.exists() && !no_link(&parent)?.is_dir() {
            return Err("Unsafe backup directory.".into());
        }
        if !parent.exists() && fs::symlink_metadata(&parent).is_ok() {
            return Err("Unsafe backup link.".into());
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "System clock precedes the Unix epoch.")?
            .as_nanos();
        let backup = parent.join(format!("w86-{stamp}-{}", std::process::id()));
        let arguments = launch_arguments(
            &text(&root, "FreeRealms.bat")?,
            endpoint,
            login.session(endpoint)?,
        )?;
        Ok(Self {
            root,
            changes,
            arguments,
            backup,
            assets,
        })
    }

    pub fn changed_files(&self) -> impl Iterator<Item = &str> {
        self.changes.iter().map(|change| change.relative.as_str())
    }

    /// Back up everything before replacing anything. Each replace is atomic;
    /// ordinary I/O failure rolls back completed writes. Backups survive errors.
    /// (R) Concurrent hostile local mutation and power-loss transactions remain
    /// outside this skeleton's boundary; use a private, user-owned client copy.
    pub fn apply(&self) -> Result<PathBuf> {
        if !no_link(&self.root.join("locale"))?.is_dir() {
            return Err("Unsafe locale directory.".into());
        }
        for change in &self.changes {
            let target = safe_target(&self.root, &change.relative)?;
            let current = if target.exists() {
                Some(bounded_read(&target, LOCALE_LIMIT)?)
            } else {
                None
            };
            if current != change.original {
                return Err("Client files changed since preparation. Prepare again.".into());
            }
        }
        let parent = self.backup.parent().ok_or("Missing backup parent")?;
        if !parent.exists() {
            fs::create_dir(parent).map_err(io_error)?;
        }
        if !no_link(parent)?.is_dir()
            || !parent
                .canonicalize()
                .map_err(io_error)?
                .starts_with(&self.root)
        {
            return Err("Unsafe backup path.".into());
        }
        fs::create_dir(&self.backup).map_err(io_error)?;
        for change in &self.changes {
            if let Some(original) = &change.original {
                let backup = self.backup.join(&change.relative);
                if let Some(parent) = backup.parent() {
                    fs::create_dir_all(parent).map_err(io_error)?;
                }
                write_new(&backup, original)?;
            }
        }
        for (index, change) in self.changes.iter().enumerate() {
            if let Err(error) = self.replace(change, &change.contents) {
                let mut rollback_ok = true;
                for previous in self.changes[..index].iter().rev() {
                    let result = if let Some(original) = &previous.original {
                        self.replace(previous, original)
                    } else {
                        fs::remove_file(self.root.join(&previous.relative)).map_err(io_error)
                    };
                    rollback_ok &= result.is_ok();
                }
                return Err(if rollback_ok {
                    error
                } else {
                    "Preparation and rollback failed. Restore .friend-launcher-backups before playing.".into()
                });
            }
        }
        Ok(self.backup.clone())
    }

    fn replace(&self, change: &Change, bytes: &[u8]) -> Result<()> {
        let target = safe_target(&self.root, &change.relative)?;
        let temporary = self.backup.join("replace.tmp");
        write_new(&temporary, bytes)?;
        if fs::rename(&temporary, target).is_err() {
            let _ = fs::remove_file(&temporary);
            return Err("Could not replace a client file; existing backups were preserved.".into());
        }
        Ok(())
    }

    /// Explicitly called only from Play. Tests and screenshot mode never call it.
    /// Keep the returned responder as long as the client runs: it serves its assets.
    pub fn launch(self) -> Result<(Child, assets::Responder)> {
        #[cfg(not(windows))]
        {
            Err("Launching Free Realms requires Windows.".into())
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let executable = regular(&self.root, "FreeRealms.exe")?;
            Command::new(executable).args(&self.arguments).current_dir(&self.root)
                .creation_flags(0x08000000) // CREATE_NO_WINDOW; no shell/batch execution.
                .spawn().map(|child| (child, self.assets)).map_err(|_| "Could not start the supplied client. Prepared files and backups were preserved.".into())
        }
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(io_error)
}

#[cfg(test)]
mod tests;
