//! Full asset install (owner 2026-11-04: install everything at install time, no streaming in play).
//!
//! The community client ships no `AssetsW_*` packs. The game fetches the rest of its asset
//! manifest (about 94,000 files) from the community CDN on first use and appends each download,
//! raw and under its plain name, to its own `AssetsW_NNN.pack`. A synchronous load (a minigame
//! movie) fails while that download is still running: the game counts an asset as local only when
//! a pack it indexes holds it with the manifest's crc (client `0x0109b310`; packs are opened as
//! `<base>_000.pack`, `_001`, ... until three in a row are missing, `0x010fd4b0`). Tower Defense's
//! "gfx not found" was exactly that.
//!
//! This step does what streaming would have done, ahead of time: every manifest asset no pack
//! holds is fetched from the community CDN, checked against the manifest's crc (and its size for a
//! plain row), and written raw under its plain name into new `AssetsW_NNN.pack` files. A rerun
//! skips what the packs already hold, so a cancelled install resumes.
use crate::{
    Result,
    character_select::entries,
    gm_tools::{crc32, write_pack_to},
    patch::{self, PatchReport, Progress, Url},
};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
};

/// The community CDN the game streams from (the launcher writes its http form into Baseline.ini).
const ASSETS: &str = "https://osfr.editz.dev/assets";
const MANIFEST: &str = "Assets_manifest.txt";
const ENVELOPE: u32 = 0xA1B2C3D4;
/// (R) The game's own `AssetsW` packs stop near 200 MB.
const PACK_BYTES: usize = 200 * 1024 * 1024;
/// (R) Parallel downloads; the CDN is a volunteer host.
const WORKERS: usize = 6;
/// (R) The largest asset is a few MB; the manifest about 9.5 MB.
const ASSET_LIMIT: u64 = 64 * 1024 * 1024;

/// One manifest row: the name the CDN serves (`wire`, `.z` = the A1B2C3D4 envelope), the plain
/// name the packs hold, the raw crc, and the raw size when the row is plain.
#[derive(Debug, PartialEq)]
pub(crate) struct Row {
    wire: String,
    name: String,
    crc: u32,
    size: Option<u64>,
}

/// Rows of `Assets_manifest.txt` (`name,crc,size`); a malformed or unsafe row is skipped.
pub(crate) fn rows(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.trim().split(',');
            let wire = parts.next()?;
            let crc = parts.next()?.parse().ok()?;
            let size: u64 = parts.next()?.parse().ok()?;
            let safe = !wire.is_empty()
                && wire.len() <= 255
                && !wire.starts_with('.')
                && wire
                    .bytes()
                    .all(|b| b > 32 && b != 127 && !b"/\\:".contains(&b) || b == b' ');
            if !safe {
                return None;
            }
            let (name, size) = match wire.strip_suffix(".z") {
                Some(name) => (name, None),
                None => (wire, Some(size)),
            };
            Some(Row {
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
            .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
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

/// (lowercase name, crc) of every entry in the client's packs, its own `assets/` cache included.
pub(crate) fn present(client: &Path) -> io::Result<HashSet<(String, u32)>> {
    let mut held = HashSet::new();
    for folder in [client.to_path_buf(), client.join("assets")] {
        let Ok(listing) = fs::read_dir(&folder) else {
            continue;
        };
        for pack in listing.filter_map(|e| e.ok().map(|e| e.path())) {
            if pack
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pack"))
            {
                for (name, _, _, crc) in entries(&pack)? {
                    held.insert((String::from_utf8_lossy(&name).to_ascii_lowercase(), crc));
                }
            }
        }
    }
    Ok(held)
}

/// The first `AssetsW_NNN.pack` number not on disk, so the client's open loop (three missing in a
/// row end it) reaches every pack.
pub(crate) fn next_pack(client: &Path) -> u32 {
    (0..1000)
        .find(|n| !client.join(format!("AssetsW_{n:03}.pack")).exists())
        .unwrap_or(1000)
}

fn write_pack(client: &Path, number: &mut u32, batch: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let path = client.join(format!("AssetsW_{:03}.pack", *number));
    let part = path.with_extension("pack.osfr-part");
    let mut out = BufWriter::new(File::create(&part).map_err(|e| e.to_string())?);
    write_pack_to(&mut out, batch).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    fs::rename(&part, &path).map_err(|e| e.to_string())?;
    *number += 1;
    batch.clear();
    Ok(())
}

fn fetch_asset(row: &Row, cancel: &AtomicBool) -> Result<Option<Vec<u8>>> {
    let url = asset_url(&row.wire)?;
    let mut error = String::new();
    for attempt in 0..3 {
        match patch::fetch_optional(&url, ASSET_LIMIT, cancel) {
            Ok(None) => return Ok(None),
            Ok(Some(body)) => match decode(body, row.size.is_none()) {
                Ok(raw)
                    if crc32(&raw) == row.crc && row.size.is_none_or(|s| s == raw.len() as u64) =>
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

/// Download every asset the client's packs lack (see the module docs). The manifest is the
/// client's own `Assets_manifest.txt` when present (GM tools add rows to it), else the CDN's,
/// written in its place: the game would fetch that same file at its first start.
pub fn install(
    client: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> Result<PatchReport> {
    let local = client.join(MANIFEST);
    let manifest = if local.is_file() {
        fs::read(&local).map_err(|e| e.to_string())?
    } else {
        let url = Url::parse(&format!("{ASSETS}/manifest.txt.z"))?;
        let body = patch::fetch_bytes(&url, ASSET_LIMIT, cancel)?;
        let text = decode(body, true)?;
        let part = local.with_extension("txt.osfr-part");
        fs::write(&part, &text).map_err(|e| e.to_string())?;
        fs::rename(&part, &local).map_err(|e| e.to_string())?;
        text
    };
    let held = present(client).map_err(|e| e.to_string())?;
    let missing: Vec<Row> = rows(&String::from_utf8_lossy(&manifest))
        .into_iter()
        .filter(|row| !held.contains(&(row.name.to_ascii_lowercase(), row.crc)))
        .collect();
    let mut status = Progress {
        phase: "Installing game assets into",
        files_total: missing.len(),
        bytes_total: missing.iter().map(|r| r.size.unwrap_or(0)).sum(),
        ..Default::default()
    };
    progress(&status);
    let (mut report, mut failed) = (PatchReport::default(), 0usize);
    let (mut number, mut batch, mut batch_bytes) = (next_pack(client), Vec::new(), 0usize);
    let next = AtomicUsize::new(0);
    let (sender, results) = mpsc::sync_channel(WORKERS * 2);
    let sender = Mutex::new(sender);
    std::thread::scope(|scope| -> Result<()> {
        for _ in 0..WORKERS {
            let (next, missing, sender) = (&next, &missing, sender.lock().unwrap().clone());
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
            status.files_done += 1;
            status.current_file = missing[index].name.clone();
            match result {
                Ok(Some(raw)) => {
                    report.files_downloaded += 1;
                    report.bytes_downloaded += raw.len() as u64;
                    status.bytes_done += raw.len() as u64;
                    batch_bytes += raw.len();
                    batch.push((missing[index].name.clone(), raw));
                    if batch_bytes >= PACK_BYTES {
                        write_pack(client, &mut number, &mut batch)?;
                        batch_bytes = 0;
                    }
                }
                Ok(None) => report.unavailable += 1,
                Err(_) if cancel.load(Ordering::SeqCst) => {}
                Err(_) => failed += 1,
            }
            progress(&status);
        }
        Ok(())
    })?;
    // What arrived is kept even when cancelled, so the next run resumes after it.
    write_pack(client, &mut number, &mut batch)?;
    patch::cancelled(cancel)?;
    if failed > 0 {
        return Err(format!(
            "{failed} game assets could not be downloaded. Download again to resume."
        ));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
    fn rows_strip_the_envelope_suffix_and_skip_unsafe_names() {
        let parsed = rows(
            "ChessAssets.txt.z,3490012822,2096\r\ntowerdefense.gfx,558485938,2367698\n../x,1,2\nbad,row\n",
        );
        assert_eq!(
            parsed,
            [
                Row {
                    wire: "ChessAssets.txt.z".into(),
                    name: "ChessAssets.txt".into(),
                    crc: 3490012822,
                    size: None
                },
                Row {
                    wire: "towerdefense.gfx".into(),
                    name: "towerdefense.gfx".into(),
                    crc: 558485938,
                    size: Some(2367698)
                },
            ]
        );
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

    #[test]
    fn written_packs_are_found_and_numbered_after_the_existing_ones() {
        let client = scratch("full-assets");
        assert_eq!(next_pack(&client), 0);
        let (mut number, mut batch) = (0, vec![("towerdefense.gfx".to_owned(), b"movie".to_vec())]);
        write_pack(&client, &mut number, &mut batch).unwrap();
        assert!(batch.is_empty());
        assert_eq!((number, next_pack(&client)), (1, 1));
        let held = present(&client).unwrap();
        assert!(held.contains(&("towerdefense.gfx".to_owned(), crc32(b"movie"))));
        let entry = &entries(&client.join("AssetsW_000.pack")).unwrap()[0];
        assert_eq!(
            (entry.0.as_slice(), entry.2),
            (b"towerdefense.gfx".as_slice(), 5)
        );
    }

    /// Live run against the community CDN on a test client folder:
    /// `OSFR_FULL_ASSETS_CLIENT=<folder> cargo test -- --ignored live_install --nocapture`.
    #[test]
    #[ignore = "downloads from the community CDN"]
    fn live_install() {
        let client = PathBuf::from(std::env::var_os("OSFR_FULL_ASSETS_CLIENT").expect("set OSFR_FULL_ASSETS_CLIENT"));
        let report = install(&client, &AtomicBool::new(false), |_| {}).unwrap();
        eprintln!(
            "{} files, {} bytes, {} unavailable",
            report.files_downloaded, report.bytes_downloaded, report.unavailable
        );
    }
}
