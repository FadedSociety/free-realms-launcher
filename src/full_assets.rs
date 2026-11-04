//! Full asset install (owner 2026-11-04: install everything at install time, no streaming in play).
//!
//! The community client ships no `AssetsW_*` packs. The game fetches the rest of its asset
//! manifest (about 94,000 files) from the community CDN on first use. A synchronous load (a
//! minigame movie) fails while that download is still running: the game counts an asset as local
//! only when a pack it loaded holds it with the manifest's crc (client `0x0109b310`). Tower
//! Defense's "gfx not found" was exactly that.
//!
//! This step installs every manifest asset no pack holds, from the community CDN, before play.
//! Pack facts (client exe, `lab` research note `asset-install.md`):
//! - Packs load as `<set>_000.pack`, `_001`, ... until three numbers in a row are missing
//!   (`0x010fd4b0`), first the read-only `Assets` group, then the client's own writable `AssetsW`.
//!   We add packs to the read-only group: the client never appends to or purges those, so nothing
//!   races the game.
//! - The client reads exactly 8,192 bytes per directory block (`0x010fc380`): an entry that ends
//!   past that window is invisible. Each block here is filled while `8192 - used > 16 + name length`
//!   and padded to 8,192 bytes. Block 0 sits at offset 0, the data follows, further blocks are
//!   appended after the data, each `next` the absolute offset of the next block.
//! - Offsets are signed 32-bit: a pack must stay under 1.5 GB (the client's deterministic limit).
//! - Names compare case-insensitively; a `.z` row is stored raw under the name without `.z`.
use crate::{
    Result,
    gm_tools::crc32,
    patch::{self, PatchReport, Progress, Url},
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

/// The community CDN the game streams from (the launcher writes its http form into Baseline.ini).
const ASSETS: &str = "https://osfr.editz.dev/assets";
const MANIFEST: &str = "Assets_manifest.txt";
const ENVELOPE: u32 = 0xA1B2C3D4;
const BLOCK: u64 = 8192;
/// A pack is published at this size. The client's own limit is 1.5 GB (offsets are signed 32-bit);
/// a smaller pack loses less to a crash or a killed launcher, since only published packs survive.
const PACK_LIMIT: u64 = 500_000_000;
/// (R) Parallel downloads; the CDN is a volunteer host.
const WORKERS: usize = 6;
/// (R) The largest asset is about 11 MB; the manifest about 9.5 MB.
const ASSET_LIMIT: u64 = 64 * 1024 * 1024;
const PART: &str = "osfr-part";
/// The rows the CDN answered 404 for, kept beside the packs so a rerun does not ask again (about
/// 37,000 of the manifest's rows are not on the community host; the game cannot get them either).
/// The first line is the crc of the manifest they were checked against: a new manifest checks
/// them all again.
const UNAVAILABLE: &str = "osfr-unavailable.txt";

/// One manifest row: the name the CDN serves (`wire`, `.z` = the A1B2C3D4 envelope), the plain
/// name the packs hold, the raw crc, and the size column (raw for a plain row, compressed for `.z`).
#[derive(Debug, PartialEq)]
pub(crate) struct Row {
    wire: String,
    name: String,
    crc: u32,
    size: u64,
}

impl Row {
    fn packed(&self) -> bool {
        self.wire.len() != self.name.len()
    }
}

/// Rows of `Assets_manifest.txt` (`name,crc,size`); a malformed row is skipped. Names never reach
/// the filesystem (they are pack entries and percent-encoded URL segments).
pub(crate) fn rows(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.trim().split(',');
            let wire = parts.next()?;
            let crc = parts.next()?.parse().ok()?;
            let size = parts.next()?.parse().ok()?;
            let safe = !wire.is_empty()
                && wire.len() <= 4096
                && wire != "."
                && wire != ".."
                && wire
                    .bytes()
                    .all(|b| (b >= 32 && b != 127) && !b"/\\".contains(&b));
            let name = wire.strip_suffix(".z").unwrap_or(wire);
            (safe && !name.is_empty()).then(|| Row {
                wire: wire.into(),
                name: name.into(),
                crc,
                size,
            })
        })
        .collect()
}

/// The CDN folder for an on-wire name: client `0x00705B00` (via the path builder `0x010A09C0`), an
/// uppercase Jenkins one-at-a-time variant, signed 32-bit, `% 1000`.
pub(crate) fn bucket(wire: &str) -> u32 {
    let mut h: i32 = 0;
    for c in wire.bytes() {
        let c = i32::from(c.to_ascii_uppercase());
        let v = h.wrapping_add(c).wrapping_mul(1025);
        h = (v >> 6) ^ v;
    }
    let h9 = h.wrapping_mul(9);
    (h9 ^ (h9 >> 11)).wrapping_mul(32769) as u32 % 1000
}

fn asset_url(wire: &str) -> Result<Url> {
    let encoded: String = wire
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    Url::parse(&format!("{ASSETS}/{:03}/{encoded}", bucket(wire)))
}

/// A `.z` body is the A1B2C3D4 envelope (big-endian magic and raw length, then zlib).
pub(crate) fn decode(body: Vec<u8>, packed: bool) -> Result<Vec<u8>> {
    if !packed {
        return Ok(body);
    }
    let bad = || "The asset host sent a damaged file.".to_owned();
    let header = |at: usize| {
        body.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (Some(ENVELOPE), Some(length)) = (header(0), header(4)) else {
        return Err(bad());
    };
    if u64::from(length) > ASSET_LIMIT {
        return Err(bad());
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&body[8..], length as usize)
        .map_err(|_| bad())?;
    if raw.len() == length as usize {
        Ok(raw)
    } else {
        Err(bad())
    }
}

/// The entries the client can see in one pack, as (uppercase name, crc): each directory block is
/// read as the client reads it, 8,192 bytes, and an entry that ends past that window is skipped.
/// A pack shorter than one block is empty to the client.
pub(crate) fn visible(pack: &Path) -> io::Result<Vec<(String, u32)>> {
    let mut file = File::open(pack)?;
    let length = file.metadata()?.len();
    let (mut at, mut out, mut seen) = (0u64, Vec::new(), 0);
    while length >= BLOCK && at <= length - 8 && seen < 100_000 {
        seen += 1;
        let mut block = vec![0; BLOCK.min(length - at) as usize];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut block)?;
        let word = |i: usize| {
            block
                .get(i..i + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        let (Some(next), Some(count)) = (word(0), word(4)) else {
            break;
        };
        let mut used = 8;
        for _ in 0..count.min(4096) {
            let Some(name_len) = word(used).map(|n| n as usize) else {
                break;
            };
            let end = used + 4 + name_len + 12;
            if name_len == 0 || end > block.len() {
                break;
            }
            let name =
                String::from_utf8_lossy(&block[used + 4..used + 4 + name_len]).to_ascii_uppercase();
            out.push((name, word(end - 4).unwrap_or(0)));
            used = end;
        }
        if next == 0 || u64::from(next) <= at {
            break;
        }
        at = next.into();
    }
    Ok(out)
}

/// The packs the client loads from `client`, in its order: `<set>_NNN.pack` for `Assets`, then
/// `AssetsW`, each until three numbers in a row are missing. Also the next free `Assets` number.
fn packs(client: &Path) -> (Vec<PathBuf>, u32) {
    let (mut found, mut next) = (Vec::new(), 0);
    for set in ["Assets", "AssetsW"] {
        let (mut index, mut gap) = (0u32, 0);
        while gap < 3 {
            let path = client.join(format!("{set}_{index:03}.pack"));
            if path.is_file() {
                found.push(path);
                gap = 0;
                if set == "Assets" {
                    next = index + 1;
                }
            } else {
                gap += 1;
            }
            index += 1;
        }
    }
    (found, next)
}

/// (uppercase name -> crcs) the client can see; an unreadable pack is skipped, as the client
/// would fail to load it too.
fn held(client: &Path) -> HashMap<String, Vec<u32>> {
    let mut held: HashMap<String, Vec<u32>> = HashMap::new();
    for pack in packs(client).0 {
        for (name, crc) in visible(&pack).unwrap_or_default() {
            held.entry(name).or_default().push(crc);
        }
    }
    held
}

/// One new read-only pack being written: block 0 reserved at offset 0, data appended as it
/// arrives, the directory written when it is finished.
struct PackOut {
    part: PathBuf,
    path: PathBuf,
    file: File,
    end: u64,
    entries: Vec<(String, u32, u32, u32)>,
}

impl PackOut {
    fn create(client: &Path, number: u32) -> Result<Self> {
        let path = client.join(format!("Assets_{number:03}.pack"));
        let part = path.with_extension(format!("pack.{PART}"));
        let mut file = File::create(&part).map_err(|e| e.to_string())?;
        file.write_all(&[0; BLOCK as usize])
            .map_err(|e| e.to_string())?;
        Ok(Self {
            part,
            path,
            file,
            end: BLOCK,
            entries: Vec::new(),
        })
    }

    /// Directory blocks the entries need under the client's fill rule.
    fn blocks(entries: &[(String, u32, u32, u32)]) -> Vec<Vec<usize>> {
        let mut blocks = vec![Vec::new()];
        let mut used = 8;
        for (i, (name, ..)) in entries.iter().enumerate() {
            if BLOCK as usize - used <= 16 + name.len() {
                blocks.push(Vec::new());
                used = 8;
            }
            if let Some(block) = blocks.last_mut() {
                block.push(i);
            }
            used += 16 + name.len();
        }
        blocks
    }

    /// Room for `size` more bytes and the blocks the pack would then need.
    fn fits(&self, name: &str, size: u64) -> bool {
        let blocks = (self.entries.len() as u64 + 1) * (16 + name.len() as u64) / (BLOCK - 512) + 2;
        self.end + size + blocks * BLOCK <= PACK_LIMIT
    }

    fn add(&mut self, name: &str, data: &[u8]) -> Result<()> {
        self.file.write_all(data).map_err(|e| e.to_string())?;
        self.entries.push((
            name.to_owned(),
            self.end as u32,
            data.len() as u32,
            crc32(data),
        ));
        self.end += data.len() as u64;
        Ok(())
    }

    /// Write the directory, sync, and publish under a name nothing holds yet. An empty pack is
    /// discarded.
    fn finish(mut self) -> Result<()> {
        if self.entries.is_empty() {
            drop(self.file);
            let _ = fs::remove_file(&self.part);
            return Ok(());
        }
        let blocks = Self::blocks(&self.entries);
        let offsets: Vec<u64> = (0..blocks.len())
            .map(|k| {
                if k == 0 {
                    0
                } else {
                    self.end + (k as u64 - 1) * BLOCK
                }
            })
            .collect();
        let render = |k: usize| {
            let next = offsets.get(k + 1).copied().unwrap_or(0) as u32;
            let mut out = Vec::with_capacity(BLOCK as usize);
            out.extend(next.to_be_bytes());
            out.extend((blocks[k].len() as u32).to_be_bytes());
            for &i in &blocks[k] {
                let (name, offset, size, crc) = &self.entries[i];
                out.extend((name.len() as u32).to_be_bytes());
                out.extend(name.as_bytes());
                out.extend(offset.to_be_bytes());
                out.extend(size.to_be_bytes());
                out.extend(crc.to_be_bytes());
            }
            out.resize(BLOCK as usize, 0);
            out
        };
        let io = |e: io::Error| e.to_string();
        for k in 1..blocks.len() {
            self.file.write_all(&render(k)).map_err(io)?;
        }
        self.file.seek(SeekFrom::Start(0)).map_err(io)?;
        self.file.write_all(&render(0)).map_err(io)?;
        self.file.sync_all().map_err(io)?;
        drop(self.file);
        // Publish without replacing: a pack that appeared meanwhile is never overwritten.
        fs::hard_link(&self.part, &self.path).map_err(io)?;
        fs::remove_file(&self.part).map_err(io)
    }
}

fn fetch_asset(row: &Row, cancel: &AtomicBool) -> Result<Option<Vec<u8>>> {
    let url = asset_url(&row.wire)?;
    // (R) 30 s plus 32 KB/s of the listed size, so slow lines finish large files.
    let time = Duration::from_secs(30 + row.size / (32 * 1024));
    let mut error = String::new();
    for attempt in 0..3 {
        match patch::fetch_optional(&url, ASSET_LIMIT, time, cancel) {
            Ok(None) => return Ok(None),
            Ok(Some(body)) => match decode(body, row.packed()) {
                Ok(raw)
                    if crc32(&raw) == row.crc && (row.packed() || row.size == raw.len() as u64) =>
                {
                    return Ok(Some(raw));
                }
                Ok(_) => error = format!("{} does not match the asset manifest.", row.name),
                Err(e) => error = e,
            },
            Err(e) => error = e,
        }
        patch::backoff(attempt, cancel)?;
    }
    Err(error)
}

/// The CDN's manifest (what the game would fetch at its first start); the client's own copy when
/// the CDN cannot be reached. A missing local copy is written from the CDN's.
fn manifest(client: &Path, cancel: &AtomicBool) -> Result<Vec<u8>> {
    let local = client.join(MANIFEST);
    let url = Url::parse(&format!("{ASSETS}/manifest.txt.z"))?;
    let mut error = String::new();
    for attempt in 0..3 {
        match patch::fetch_bytes(&url, ASSET_LIMIT, cancel).and_then(|body| decode(body, true)) {
            Ok(text) => {
                if !local.is_file() {
                    let part = local.with_extension(format!("txt.{PART}"));
                    fs::write(&part, &text).map_err(|e| e.to_string())?;
                    fs::rename(&part, &local).map_err(|e| e.to_string())?;
                }
                return Ok(text);
            }
            Err(e) => error = e,
        }
        patch::backoff(attempt, cancel)?;
    }
    fs::read(&local).map_err(|_| error)
}

/// The wire names a run under the same manifest found missing on the CDN.
fn unavailable(client: &Path, manifest_crc: u32) -> HashSet<String> {
    let Ok(text) = fs::read_to_string(client.join(UNAVAILABLE)) else {
        return HashSet::new();
    };
    let mut lines = text.lines();
    if lines.next() != Some(manifest_crc.to_string().as_str()) {
        return HashSet::new();
    }
    lines.map(str::to_owned).collect()
}

fn remember_unavailable<'a>(
    client: &Path,
    manifest_crc: u32,
    wires: impl Iterator<Item = &'a str>,
) -> Result<()> {
    let mut text = format!("{manifest_crc}\n");
    for wire in wires {
        text.push_str(wire);
        text.push('\n');
    }
    let path = client.join(UNAVAILABLE);
    let part = path.with_extension(format!("txt.{PART}"));
    fs::write(&part, text).map_err(|e| e.to_string())?;
    fs::rename(&part, &path).map_err(|e| e.to_string())
}

/// Install every manifest asset the client cannot already see (see the module docs). A rerun
/// installs only what is still missing, so a cancelled or failed install resumes; rows the CDN
/// did not have under this manifest are not asked for again.
pub fn install(
    client: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> Result<PatchReport> {
    for stale in fs::read_dir(client).map_err(|e| e.to_string())?.flatten() {
        if stale
            .file_name()
            .to_string_lossy()
            .ends_with(&format!(".pack.{PART}"))
        {
            let _ = fs::remove_file(stale.path());
        }
    }
    let manifest = manifest(client, cancel)?;
    let manifest_crc = crc32(&manifest);
    let mut gone = unavailable(client, manifest_crc);
    let held = held(client);
    let (mut conflicts, mut known_gone) = (0, 0);
    let missing: Vec<Row> = rows(&String::from_utf8_lossy(&manifest))
        .into_iter()
        .filter(|row| match held.get(&row.name.to_ascii_uppercase()) {
            None if gone.contains(&row.wire) => {
                known_gone += 1;
                false
            }
            None => true,
            Some(crcs) if crcs.contains(&row.crc) => false,
            // A visible copy with another crc cannot be shadowed safely (asset-install.md Q1).
            Some(_) => {
                conflicts += 1;
                false
            }
        })
        .collect();
    let mut status = Progress {
        phase: "Installing game assets into",
        files_total: missing.len(),
        bytes_total: missing.iter().map(|r| r.size).sum(),
        ..Default::default()
    };
    progress(&status);
    let (mut report, mut failed) = (PatchReport::default(), 0usize);
    let mut number = packs(client).1;
    let mut pack: Option<PackOut> = None;
    let next = AtomicUsize::new(0);
    let (sender, results) = mpsc::sync_channel(WORKERS * 2);
    let sender = Mutex::new(sender);
    let written = std::thread::scope(|scope| -> Result<()> {
        for _ in 0..WORKERS {
            let sender = sender
                .lock()
                .map_err(|_| "Worker setup failed.".to_owned())?
                .clone();
            let (next, missing) = (&next, &missing);
            scope.spawn(move || {
                while !cancel.load(Ordering::SeqCst) {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(row) = missing.get(index) else { break };
                    if sender.send((index, fetch_asset(row, cancel))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (index, result) in results {
            let row = &missing[index];
            status.files_done += 1;
            status.bytes_done += row.size;
            status.current_file = row.name.clone();
            match result {
                Ok(Some(raw)) => {
                    if pack
                        .as_ref()
                        .is_some_and(|p| !p.fits(&row.name, raw.len() as u64))
                    {
                        pack.take().map(PackOut::finish).transpose()?;
                        number += 1;
                    }
                    if pack.is_none() {
                        while client.join(format!("Assets_{number:03}.pack")).exists() {
                            number += 1;
                        }
                        pack = Some(PackOut::create(client, number)?);
                    }
                    if let Some(pack) = pack.as_mut() {
                        pack.add(&row.name, &raw)?;
                    }
                    report.files_downloaded += 1;
                    report.bytes_downloaded += raw.len() as u64;
                }
                Ok(None) => {
                    report.unavailable += 1;
                    gone.insert(row.wire.clone());
                    // Saved as it grows, so a crash does not cost the walk through them again.
                    if report.unavailable % 1000 == 0 {
                        let wires = gone.iter().map(String::as_str);
                        remember_unavailable(client, manifest_crc, wires)?;
                    }
                }
                Err(_) if cancel.load(Ordering::SeqCst) => {}
                Err(_) => failed += 1,
            }
            progress(&status);
        }
        Ok(())
    });
    // What arrived is published even after a cancel or an error, so the next run resumes after it.
    let finished = pack.map(PackOut::finish).transpose();
    let remembered = remember_unavailable(client, manifest_crc, gone.iter().map(String::as_str));
    written?;
    finished?;
    remembered?;
    patch::cancelled(cancel)?;
    report.unavailable += conflicts + known_gone;
    if failed > 0 {
        return Err(format!(
            "{failed} game assets could not be downloaded. Download again to finish."
        ));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".test-data")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn buckets_match_the_client_hash() {
        // Values the community CDN serves at (checked 2026-11-04 against lab/tools/cdn_mirror_fetch.py).
        assert_eq!(bucket("towerdefense.gfx"), 952);
        assert_eq!(bucket("Mauthdog final.gfx"), 446);
        assert_eq!(bucket("ChessAssets.txt.z"), 717);
        assert!(asset_url("Mauthdog final.gfx").is_ok());
    }

    #[test]
    fn rows_strip_the_envelope_suffix_and_skip_bad_names() {
        let parsed = rows(
            "ChessAssets.txt.z,3490012822,2096\r\n..dma.z,1,2\ntowerdefense.gfx,558485938,2367698\na/b,1,2\nbad,row\n",
        );
        assert_eq!(
            parsed
                .iter()
                .map(|r| (r.name.as_str(), r.packed()))
                .collect::<Vec<_>>(),
            [
                ("ChessAssets.txt", true),
                ("..dma", true),
                ("towerdefense.gfx", false)
            ]
        );
        assert_eq!((parsed[0].crc, parsed[0].size), (3490012822, 2096));
    }

    #[test]
    fn an_envelope_decodes_and_a_bad_one_is_refused() {
        let raw = b"hello pack".to_vec();
        let mut body = ENVELOPE.to_be_bytes().to_vec();
        body.extend((raw.len() as u32).to_be_bytes());
        body.extend(miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
        assert_eq!(decode(body.clone(), true).unwrap(), raw);
        body[0] = 0;
        assert!(decode(body, true).is_err());
        assert_eq!(decode(raw.clone(), false).unwrap(), raw);
    }

    /// Thousands of entries: every one must be visible through the client's 8,192-byte window, and
    /// the pack lands after the existing read-only ones.
    #[test]
    fn missing_rows_are_remembered_for_their_manifest_only() {
        let client = scratch("full-assets-unavailable");
        assert!(unavailable(&client, 7).is_empty());
        remember_unavailable(&client, 7, ["a.dds.z", "b.mp3"].into_iter()).unwrap();
        let gone = unavailable(&client, 7);
        assert!(gone.contains("a.dds.z") && gone.contains("b.mp3") && gone.len() == 2);
        assert!(
            unavailable(&client, 8).is_empty(),
            "a new manifest checks them again"
        );
    }

    #[test]
    fn a_big_pack_keeps_every_entry_visible() {
        let client = scratch("full-assets");
        fs::write(client.join("Assets_000.pack"), [0u8; 10]).unwrap();
        fs::write(client.join("Assets_001.pack"), [0u8; 10]).unwrap();
        assert_eq!(packs(&client).1, 2);
        let mut pack = PackOut::create(&client, 2).unwrap();
        let names: Vec<String> = (0..5000)
            .map(|i| format!("icon_item_freestyle_{i:05}_64.dds"))
            .collect();
        for (i, name) in names.iter().enumerate() {
            pack.add(name, &(i as u32).to_le_bytes()).unwrap();
        }
        pack.finish().unwrap();
        let seen = visible(&client.join("Assets_002.pack")).unwrap();
        assert_eq!(seen.len(), names.len());
        assert_eq!(
            seen[4999],
            (
                names[4999].to_ascii_uppercase(),
                crc32(&4999u32.to_le_bytes())
            )
        );
        assert!(held(&client).contains_key(&names[0].to_ascii_uppercase()));
        // The next pack number skips the one just written; nothing is replaced.
        assert_eq!(packs(&client).1, 3);
        let mut again = PackOut::create(&client, 2).unwrap();
        again.add("x", b"y").unwrap();
        assert!(again.finish().is_err());
        assert_eq!(
            visible(&client.join("Assets_002.pack")).unwrap().len(),
            5000
        );
    }

    #[test]
    fn an_entry_past_the_window_is_not_visible() {
        let client = scratch("full-assets-window");
        // One block with 600 entries of 20-byte names: about 21 KB of directory, so only the
        // entries inside the first 8,192 bytes count, as in the stock packs the client half reads.
        let mut block = vec![0, 0, 0, 0];
        block.extend(600u32.to_be_bytes());
        for i in 0..600u32 {
            let name = format!("name_{i:015}");
            block.extend((name.len() as u32).to_be_bytes());
            block.extend(name.as_bytes());
            block.extend([0u8; 12]);
        }
        block.resize(block.len().max(BLOCK as usize), 0);
        fs::write(client.join("Assets_000.pack"), &block).unwrap();
        let seen = visible(&client.join("Assets_000.pack")).unwrap();
        assert_eq!(seen.len(), (8192 - 8) / 36);
    }

    /// Live run against the community CDN on a test client folder:
    /// `OSFR_FULL_ASSETS_CLIENT=<folder> cargo test -- --ignored live_install --nocapture`.
    #[test]
    #[ignore = "downloads from the community CDN"]
    fn live_install() {
        let client = PathBuf::from(
            std::env::var_os("OSFR_FULL_ASSETS_CLIENT").expect("set OSFR_FULL_ASSETS_CLIENT"),
        );
        let report = install(&client, &AtomicBool::new(false), |_| {}).unwrap();
        eprintln!(
            "{} files, {} bytes, {} unavailable or conflicting",
            report.files_downloaded, report.bytes_downloaded, report.unavailable
        );
    }
}
