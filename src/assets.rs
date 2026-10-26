//! Loopback indirect-asset responder for the launched client (L024).
//!
//! The client exits with code 27 when `IndirectServerAddress` cannot deliver
//! `manifest.crc` (run 20261008T015537Z). Like the native harness
//! (scripts/run-local-baseline.py, run 20261021T193008Z) this answers
//! `manifest.crc` from the client's own `Assets_manifest.txt` and serves asset
//! bodies from `<client>/indirect-assets/`, while the `Responder` lives.
use crate::{Result, bounded_read, hard_link, io_error, patch::valid_path, regular};
use std::{
    fs::File,
    io::{self, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// A realm's patch manifest fills this folder. Files are served byte for byte
/// under the requested manifest name: a `.z` file holds the A1B2C3D4 envelope
/// the client inflates (AssetReceived 0x0109b400, envelope reader 0x010ebee0).
const FOLDER: &str = "indirect-assets";
/// (R) Bounds the manifest read into memory; the harness's is 9,479,368 bytes.
const LIMIT: u64 = 64 * 1024 * 1024;
/// (R) Connections served at once; more are closed unanswered and the client
/// retries (0x010a1980). Bounds the threads any local process can pin open.
pub(crate) const CONNECTIONS: usize = 64;
/// (R) The whole request head must arrive within this, however it is trickled.
const HEAD_TIME: Duration = Duration::from_secs(5);

/// Serves until dropped. Keep it with the client's `Child`.
pub struct Responder {
    /// `http://127.0.0.1:<port>/`, the harness's IndirectServerAddress form.
    pub url: String,
    address: SocketAddr,
    stop: Arc<AtomicBool>,
}

/// Frees a connection slot however its thread ends, or if it never starts.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Responder {
    pub fn start(root: &Path) -> Result<Self> {
        let manifest = bounded_read(&regular(root, "Assets_manifest.txt")?, LIMIT)?;
        // 0x010a1980 splits the body at ','; OnManifestCrcReceived 0x0109bd00 then
        // keeps its local `%s_manifest.txt` when its CRC (0x00704a40) matches, so the
        // client never asks for manifest.txt.z, and it never requests the manifest
        // by its local name (request strings 0x018858a0, 0x01885920). The CRLF
        // matches the harness.
        let crc: Arc<[u8]> = format!("{},{}\r\n", crc32(&manifest), manifest.len())
            .into_bytes()
            .into();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(io_error)?;
        let address = listener.local_addr().map_err(io_error)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let folder: Arc<Path> = root.join(FOLDER).into();
        let active = Arc::new(AtomicUsize::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else {
                    // A lasting accept error (WSAEMFILE) must not spin a core.
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                };
                // Only this loop adds, so the check cannot race past the cap.
                if active.load(Ordering::SeqCst) < CONNECTIONS {
                    // One thread per connection, as the harness's ThreadingHTTPServer:
                    // an idle client connection never holds up another.
                    active.fetch_add(1, Ordering::SeqCst);
                    let (folder, crc, slot) = (folder.clone(), crc.clone(), Slot(active.clone()));
                    // Builder, not spawn: a failed thread start must not end this loop.
                    let _ = std::thread::Builder::new().spawn(move || {
                        let _slot = slot;
                        let _ = serve(&stream, &folder, &crc);
                    });
                }
            }
        });
        Ok(Self {
            url: format!("http://{address}/"),
            address,
            stop,
        })
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address); // Wakes the accept loop to see the flag.
    }
}

fn serve(mut stream: &TcpStream, folder: &Path, crc: &[u8]) -> io::Result<()> {
    let start = Instant::now();
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    // Read the whole head (GetRequest 0x00d31140 ends it `Host: %s\r\n\r\n`):
    // closing on unread bytes would reset the reply. One absolute deadline, as
    // auth.rs `remaining`, so a sender that trickles bytes cannot keep its slot.
    let (mut head, mut read) = ([0; 8192], 0);
    while read < head.len() && !head[..read].windows(4).any(|w| w == b"\r\n\r\n") {
        let left = HEAD_TIME
            .checked_sub(start.elapsed())
            .filter(|d| !d.is_zero());
        stream.set_read_timeout(Some(left.ok_or(io::ErrorKind::TimedOut)?))?;
        match stream.read(&mut head[read..])? {
            0 => break,
            n => read += n,
        }
    }
    let line = head[..read]
        .split(|&b| b == b'\n')
        .next()
        .unwrap_or_default();
    let line = std::str::from_utf8(line.trim_ascii()).unwrap_or("");
    // GetRequest sends `GET <path> HTTP/1.1` with the path copied unescaped
    // (URL split 0x00d30100), so a manifest name may carry spaces.
    let found = line
        .strip_prefix("GET ")
        .and_then(|rest| rest.rsplit_once(' '))
        .and_then(|(target, _)| body(target, folder, crc));
    let status = if found.is_some() {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let (length, mut body) = found.unwrap_or((0, Box::new(io::empty())));
    write!(
        stream,
        "HTTP/1.0 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {length}\r\n\r\n"
    )?;
    io::copy(&mut body, &mut stream)?;
    // As the harness's socketserver: FIN after the reply, then close.
    stream.shutdown(Shutdown::Write)
}

/// The reply body and its length; files stream, so a held slot costs one copy buffer.
fn body<'a>(target: &str, folder: &Path, crc: &'a [u8]) -> Option<(u64, Box<dyn Read + 'a>)> {
    // Requests are the base URL plus `manifest.crc` or `%03u/<manifest name>?<crc>`
    // (0x010a1e00, 0x010a09c0); harness log: /960/sg_bixie_fence_tall_01_lod0.dme.z.
    let path = target.split('?').next()?;
    if path == "/manifest.crc" {
        return Some((crc.len() as u64, Box::new(crc)));
    }
    let (bucket, name) = path.strip_prefix('/')?.split_once('/')?;
    // One patchable file name only: no `..`, separators, `%`, `:` or devices.
    if bucket.is_empty()
        || !bucket.bytes().all(|b| b.is_ascii_digit())
        || name.contains('/')
        || !valid_path(name)
    {
        return None;
    }
    // `regular` also refuses links and anything resolving outside the folder; a
    // hard link could alias any file this user can read, so it is refused too.
    let path = regular(folder, name).ok()?;
    if hard_link(&path).ok()? {
        return None;
    }
    let file = File::open(path).ok()?;
    // `take` keeps the body to the advertised length if the file grows meanwhile.
    let length = file.metadata().ok()?.len();
    Some((length, Box::new(file.take(length))))
}

/// Standard CRC-32, the harness's `zlib.crc32`.
fn crc32(bytes: &[u8]) -> u32 {
    let table: [u32; 256] = std::array::from_fn(|n| {
        (0..8).fold(n as u32, |c, _| {
            (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg())
        })
    });
    !bytes.iter().fold(!0_u32, |c, &b| {
        table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8)
    })
}
