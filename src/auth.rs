//! (R) Per-server key-pair login, per notes/Research/Key-pair Login Contract.md.
//! This is reconstructed authentication, not the historical account service.
//! Brokers must be loopback or a private-network literal (LAN or VPN); never public.

use crate::SessionToken;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

// (R) Bound local UI/network work. Each exchange has an absolute deadline,
// including partial reads/writes; there is never an automatic proof retry.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    ServerUnreachable,
    WrongServerIdentity,
    Refused,
    TryLater,
    Timeout,
    InvalidReply,
    InvalidProfile,
    KeyStorage,
    RandomUnavailable,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ServerUnreachable => "Server unreachable. Check the broker or secure tunnel.",
            Self::WrongServerIdentity => "Wrong server identity. Check the trusted server profile.",
            // AUTH2 deliberately gives no separate unknown/disabled/locked status.
            Self::Refused => "Login refused. Check key enrolment or try again later.",
            Self::TryLater => "Server closed the login request. Try again later.",
            Self::Timeout => "Login timed out. Check the broker or try again later.",
            Self::InvalidReply => "Server returned an invalid login reply.",
            Self::InvalidProfile => "Invalid server profile. Check the trusted profile settings.",
            Self::KeyStorage => "Cannot access the private profile files. Check their permissions.",
            Self::RandomUnavailable => "The operating system could not generate a secure key.",
        })
    }
}
impl std::error::Error for AuthError {}

/// (R) Trusted local configuration. No pin is learned from a network response.
/// Profile IDs select fixed filenames, never paths supplied by a broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub display_name: String,
    pub broker_endpoint: String,
    pub server_identity: [u8; 32],
    pub account_handle: String,
}

impl Profile {
    pub fn new(
        id: &str,
        display_name: &str,
        broker_endpoint: &str,
        server_identity: [u8; 32],
        account_handle: &str,
    ) -> Result<Self, AuthError> {
        let profile = Self {
            id: id.into(),
            display_name: display_name.into(),
            broker_endpoint: broker_endpoint.into(),
            server_identity,
            account_handle: account_handle.into(),
        };
        profile.validate()?;
        Ok(profile)
    }

    fn validate(&self) -> Result<SocketAddr, AuthError> {
        valid_id(&self.id)?;
        if self.display_name.trim().is_empty()
            || self.display_name.len() > 80
            || self.display_name.chars().any(char::is_control)
            || !(3..=32).contains(&self.account_handle.len())
            || !self
                .account_handle
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(AuthError::InvalidProfile);
        }
        crate::validate_endpoint(&self.broker_endpoint).map_err(|_| AuthError::InvalidProfile)?;
        // Literal addresses avoid an unbounded system DNS resolver and rebinding.
        let address: SocketAddr = self
            .broker_endpoint
            .parse()
            .map_err(|_| AuthError::InvalidProfile)?;
        if !broker_ip_allowed(address.ip()) {
            return Err(AuthError::InvalidProfile);
        }
        Ok(address)
    }
}

/// Security Model decision 2 (2026-10-16): loopback, RFC 1918, the VPN shared
/// space 100.64.0.0/10, or IPv6 unique-local. Public, wildcard and link-local
/// addresses stay refused; AUTH2 still pins the server identity on top.
fn broker_ip_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn valid_id(id: &str) -> Result<(), AuthError> {
    // (R) Reject Windows device names as well as path syntax.
    let upper = id.to_ascii_uppercase();
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit())
    {
        return Err(AuthError::InvalidProfile);
    }
    Ok(())
}

/// The location is derived only from the OS profile, never the executable/game.
/// This performs no writes and rejects relative/missing APPDATA (no cwd fallback).
pub fn profile_directory(id: &str) -> Result<PathBuf, AuthError> {
    valid_id(id)?;
    let appdata = PathBuf::from(std::env::var_os("APPDATA").ok_or(AuthError::KeyStorage)?);
    if !appdata.is_absolute() {
        return Err(AuthError::KeyStorage);
    }
    Ok(appdata.join("FreeRealmsExpanded").join("profiles").join(id))
}

fn regular_private(path: &Path) -> Result<(), AuthError> {
    if !crate::no_link(path)
        .map_err(|_| AuthError::KeyStorage)?
        .is_file()
        || crate::hard_link(path).map_err(|_| AuthError::KeyStorage)?
    {
        return Err(AuthError::KeyStorage);
    }
    private_permissions(path, false)
}

fn directory(id: &str) -> Result<PathBuf, AuthError> {
    let directory = profile_directory(id)?;
    // Reject junctions/symlinks in every existing ancestor before creating files.
    for ancestor in directory.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {
                if !crate::no_link(ancestor)
                    .map_err(|_| AuthError::KeyStorage)?
                    .is_dir()
                {
                    return Err(AuthError::KeyStorage);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(_) => return Err(AuthError::KeyStorage),
        }
    }
    // APPDATA itself must exist; create only this application's directories.
    let profiles = directory.parent().ok_or(AuthError::KeyStorage)?;
    let app = profiles.parent().ok_or(AuthError::KeyStorage)?;
    for path in [app, profiles, directory.as_path()] {
        match create_directory(path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(AuthError::KeyStorage),
        }
        if !crate::no_link(path)
            .map_err(|_| AuthError::KeyStorage)?
            .is_dir()
        {
            return Err(AuthError::KeyStorage);
        }
        private_permissions(path, true)?;
    }
    Ok(directory)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), AuthError> {
    // Exclusive creation never rotates/overwrites an enrolled key. The parent
    // already has an owner-only inheritable ACL before an empty file is created.
    let mut file = create_file(path)?;
    let result = private_permissions(path, false).and_then(|()| {
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| AuthError::KeyStorage)
    });
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    result
}

/// Generate once, from the OS CSPRNG. Export only the 64-character public key.
pub fn new_key(id: &str) -> Result<String, AuthError> {
    let path = directory(id)?.join("key.ed25519");
    let mut seed = Zeroizing::new([0; 32]);
    getrandom::getrandom(seed.as_mut()).map_err(|_| AuthError::RandomUnavailable)?;
    let public = hex(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());
    write_private(&path, seed.as_ref())?;
    Ok(public)
}

fn read_key(id: &str) -> Result<Zeroizing<Vec<u8>>, AuthError> {
    let path = directory(id)?.join("key.ed25519");
    regular_private(&path)?;
    // Even malformed/oversized key-file bytes stay in a zeroizing buffer.
    let mut seed = Zeroizing::new(Vec::with_capacity(33));
    File::open(path)
        .map_err(|_| AuthError::KeyStorage)?
        .take(33)
        .read_to_end(&mut seed)
        .map_err(|_| AuthError::KeyStorage)?;
    if seed.len() != 32 {
        return Err(AuthError::KeyStorage);
    }
    Ok(seed)
}

pub fn print_public_key(id: &str) -> Result<String, AuthError> {
    let seed = read_key(id)?;
    let seed: &[u8; 32] = seed
        .as_slice()
        .try_into()
        .map_err(|_| AuthError::KeyStorage)?;
    Ok(hex(&SigningKey::from_bytes(seed)
        .verifying_key()
        .to_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| {
            [
                DIGITS[(b >> 4) as usize] as char,
                DIGITS[(b & 15) as usize] as char,
            ]
        })
        .collect()
}

/// Persist a new trusted profile and its separate key; refuse implicit pin or
/// account changes. Editing/re-enrolment needs an explicit future UI decision.
pub fn save_profile(profile: &Profile) -> Result<(), AuthError> {
    profile.validate()?;
    let directory = directory(&profile.id)?;
    if !directory
        .join("key.ed25519")
        .try_exists()
        .map_err(|_| AuthError::KeyStorage)?
    {
        new_key(&profile.id)?;
    }
    read_key(&profile.id)?;
    let bytes = serde_json::to_vec(profile).map_err(|_| AuthError::InvalidProfile)?;
    write_private(&directory.join("profile.json"), &bytes)
}

pub fn load_profile(id: &str) -> Result<Profile, AuthError> {
    let path = directory(id)?.join("profile.json");
    regular_private(&path)?;
    let bytes = crate::bounded_read(&path, 4096).map_err(|_| AuthError::KeyStorage)?;
    let profile: Profile = serde_json::from_slice(&bytes).map_err(|_| AuthError::InvalidProfile)?;
    profile.validate()?;
    if profile.id != id {
        return Err(AuthError::InvalidProfile);
    }
    Ok(profile)
}

fn network_error(error: io::Error) -> AuthError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => AuthError::Timeout,
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::BrokenPipe => AuthError::TryLater,
        _ => AuthError::ServerUnreachable,
    }
}

fn remaining(start: Instant) -> Result<Duration, AuthError> {
    EXCHANGE_TIMEOUT
        .checked_sub(start.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or(AuthError::Timeout)
}

fn exchange(address: SocketAddr, body: &[u8]) -> Result<Zeroizing<[u8; 69]>, AuthError> {
    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
        .map_err(|_| AuthError::ServerUnreachable)?;
    let start = Instant::now();
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend((body.len() as u32).to_le_bytes());
    frame.extend(body);
    let mut outgoing = frame.as_slice();
    while !outgoing.is_empty() {
        stream
            .set_write_timeout(Some(remaining(start)?))
            .map_err(network_error)?;
        match stream.write(outgoing) {
            Ok(0) => return Err(AuthError::TryLater),
            Ok(n) => outgoing = &outgoing[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(network_error(e)),
        }
    }
    let mut reply = Zeroizing::new([0; 69]);
    let mut read = 0;
    while read < reply.len() {
        stream
            .set_read_timeout(Some(remaining(start)?))
            .map_err(network_error)?;
        match stream.read(&mut reply[read..]) {
            Ok(0) => return Err(AuthError::TryLater),
            Ok(n) => read += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(network_error(e)),
        }
    }
    remaining(start)?;
    if reply.iter().all(|b| *b == 0) {
        return Err(AuthError::Refused);
    }
    if reply[0] != 1 {
        return Err(AuthError::InvalidReply);
    }
    Ok(reply)
}

/// Two AUTH2 exchanges, exact W94 transcript; token stays in memory. No retries.
pub fn login(profile: &Profile) -> Result<SessionToken, AuthError> {
    let address = profile.validate()?;
    // Bind selection to the persisted trusted endpoint, pin and account, rather
    // than allowing a peer or an accidental UI edit to select another key.
    if load_profile(&profile.id)? != *profile {
        return Err(AuthError::InvalidProfile);
    }
    let mut request = b"AUTH2\x01".to_vec();
    request.extend((profile.account_handle.len() as u32).to_le_bytes());
    request.extend(profile.account_handle.as_bytes());
    let challenge = exchange(address, &request)?;
    if !bool::from(challenge[1..33].ct_eq(&profile.server_identity)) {
        return Err(AuthError::WrongServerIdentity);
    }
    if challenge[65..69] != 60u32.to_le_bytes() {
        return Err(AuthError::InvalidReply);
    }
    // The key is not even loaded until the peer passes the pin check.
    let seed = read_key(&profile.id)?;
    let key = SigningKey::from_bytes(
        seed.as_slice()
            .try_into()
            .map_err(|_| AuthError::KeyStorage)?,
    );
    let mut message = b"osfr-login-v1".to_vec();
    message.extend(profile.server_identity);
    message.extend(&challenge[33..65]);
    message.extend(profile.account_handle.as_bytes());
    let mut proof = b"AUTH2\x02".to_vec();
    proof.extend(&challenge[33..65]);
    proof.extend(key.verifying_key().to_bytes());
    proof.extend(key.sign(&message).to_bytes());
    let reply = exchange(address, &proof)?;
    if !reply[1..33]
        .iter()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        || reply[33..].iter().any(|b| *b != 0)
    {
        return Err(AuthError::InvalidReply);
    }
    let token = std::str::from_utf8(&reply[1..33]).map_err(|_| AuthError::InvalidReply)?;
    SessionToken::new(token.to_owned()).map_err(|_| AuthError::InvalidReply)
}

// Reuse the existing launch token without changing the lead-owned handoff code.
// Formatting SessionId in launch_arguments borrows it, then this Drop wipes the
// original. OS/client argv copies still follow the native launch boundary.
impl Drop for SessionToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(unix)]
fn private_permissions(path: &Path, directory: bool) -> Result<(), AuthError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|_| AuthError::KeyStorage)?
        .permissions()
        .mode()
        & 0o777;
    if mode != if directory { 0o700 } else { 0o600 } {
        return Err(AuthError::KeyStorage);
    }
    Ok(())
}

#[cfg(unix)]
fn create_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(unix)]
fn create_file(path: &Path) -> Result<File, AuthError> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| AuthError::KeyStorage)
}

#[cfg(windows)]
use windows_private::{create_directory, create_file, private_permissions};

#[cfg(windows)]
mod windows_private {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr,
    };

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut c_void,
        inherit: i32,
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn GetFileSecurityW(
            path: *const u16,
            information: u32,
            descriptor: *mut c_void,
            size: u32,
            needed: *mut u32,
        ) -> i32;
        fn ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor: *const c_void,
            revision: u32,
            information: u32,
            text: *mut *mut u16,
            length: *mut u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn CreateDirectoryW(path: *const u16, attributes: *const SecurityAttributes) -> i32;
        fn CreateFileW(
            path: *const u16,
            access: u32,
            share: u32,
            attributes: *const SecurityAttributes,
            disposition: u32,
            flags: u32,
            template: *mut c_void,
        ) -> *mut c_void;
    }

    // (R) Protected owner-only DACL, including on creation. Administrators can
    // take ownership; a compromised user/admin is outside this boundary.
    fn sddl(directory: bool) -> &'static str {
        if directory {
            "D:P(A;OICI;FA;;;OW)"
        } else {
            "D:P(A;;FA;;;OW)"
        }
    }

    fn with_descriptor<T>(
        directory: bool,
        operation: impl FnOnce(*mut c_void) -> T,
    ) -> Result<T, AuthError> {
        let text: Vec<u16> = sddl(directory).encode_utf16().chain([0]).collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: Win32 allocates this descriptor from the valid UTF-16 SDDL;
        // it remains live through operation and is freed exactly once afterwards.
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            ) == 0
            {
                return Err(AuthError::KeyStorage);
            }
            let result = operation(descriptor);
            LocalFree(descriptor);
            Ok(result)
        }
    }

    fn wide(path: &Path) -> Result<Vec<u16>, AuthError> {
        let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
        if path.contains(&0) {
            return Err(AuthError::KeyStorage);
        }
        path.push(0);
        Ok(path)
    }

    pub(super) fn create_directory(path: &Path) -> io::Result<()> {
        let path = wide(path).map_err(io::Error::other)?;
        with_descriptor(true, |descriptor| {
            let attributes = SecurityAttributes {
                length: size_of::<SecurityAttributes>() as u32,
                descriptor,
                inherit: 0,
            };
            // SAFETY: valid path, attributes and descriptor for this call.
            if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
        .map_err(io::Error::other)?
    }

    pub(super) fn create_file(path: &Path) -> Result<File, AuthError> {
        let path = wide(path)?;
        with_descriptor(false, |descriptor| {
            let attributes = SecurityAttributes {
                length: size_of::<SecurityAttributes>() as u32,
                descriptor,
                inherit: 0,
            };
            // SAFETY: valid pointers; GENERIC_WRITE, no sharing, CREATE_NEW.
            // The returned unique handle transfers ownership into File once.
            let handle = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    0x40000000,
                    0,
                    &attributes,
                    1,
                    0x80,
                    ptr::null_mut(),
                )
            };
            if handle as isize == -1 {
                return Err(AuthError::KeyStorage);
            }
            Ok(unsafe { File::from_raw_handle(handle) })
        })?
    }

    pub(super) fn private_permissions(path: &Path, directory: bool) -> Result<(), AuthError> {
        let path = wide(path)?;
        let mut needed = 0;
        // SAFETY: size probe writes only needed; no descriptor buffer supplied.
        unsafe {
            GetFileSecurityW(path.as_ptr(), 4, ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 || needed > 65536 {
            return Err(AuthError::KeyStorage);
        }
        // DWORD-aligned storage for the self-relative security descriptor.
        let mut descriptor = vec![0u32; (needed as usize).div_ceil(4)];
        let mut text = ptr::null_mut();
        let mut length = 0;
        // SAFETY: adequate aligned buffer, bounded by the successful size probe.
        // The string allocation is read using its returned length and freed once.
        unsafe {
            if GetFileSecurityW(
                path.as_ptr(),
                4,
                descriptor.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ) == 0
                || ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor.as_ptr().cast(),
                    1,
                    4,
                    &mut text,
                    &mut length,
                ) == 0
            {
                return Err(AuthError::KeyStorage);
            }
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length as usize));
            LocalFree(text.cast());
            if value.trim_end_matches('\0') != sddl(directory) {
                return Err(AuthError::KeyStorage);
            }
        }
        Ok(())
    }
}

#[cfg(not(any(windows, unix)))]
fn private_permissions(_: &Path, _: bool) -> Result<(), AuthError> {
    Err(AuthError::KeyStorage)
}

#[cfg(not(any(windows, unix)))]
fn create_directory(_: &Path) -> io::Result<()> {
    Err(io::Error::other(AuthError::KeyStorage))
}

#[cfg(not(any(windows, unix)))]
fn create_file(_: &Path) -> Result<File, AuthError> {
    Err(AuthError::KeyStorage)
}

#[cfg(test)]
#[path = "../tests/support/auth_wire.rs"]
mod wire_tests;
