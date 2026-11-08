//! The card window's art. `FreeRealmsTCG.dll` asks a dead address for `manifest.crc` and waits for
//! it for ever, so its art never loads, and the thread left waiting takes the client down when Exit
//! TCG unloads the DLL. A row the pack lacks is no better: a failed fetch hands the DLL null data.
//! So the DLL reads the crc from the client's own folder instead, and its manifest keeps only the
//! rows its pack holds.
//!
//! The client's own card set has the same dead address (`TcgServerAddress`) and Go waits on it. Its
//! file list names what the card window would download, and an install already has all of that, so
//! it gets an empty list from a local folder. Nothing is fetched, and every step can run again at
//! every launch.
use crate::{
    Result,
    assets::crc32,
    patch::{self, Url},
    safe_target,
};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::AtomicBool,
};

const DLL: &str = "tcg/FreeRealmsTCG.dll";
const PACK: &str = "assets/assetpack000W_000.pack";
const MANIFEST: &str = "assets/assetpack000_manifest.txt";
/// The manifest as it shipped, kept once: the rows a later art pack brings back live here.
const STOCK: &str = "assets/assetpack000_manifest.stock.txt";
const CRC: &str = "assets/manifest.crc";
/// The address in the DLL and what replaces it, both with the end of the string.
const DEAD: &[u8] = b"http://fr.patch.station.sony.com/patch/freerealmstcg/live/assets/\0";
const LOCAL: &[u8] = b"file://./assets/\0";
/// More rows than the 9,756 of the art manifest; a pack past this is damaged.
const MAX_ROWS: u32 = 1 << 20;
/// Card faces the client's own pack lacks, as one read-only pack on our bucket (owner 2026-11-08),
/// pinned by hash. `ART_ROWS` are its `name,crc,size` rows.
const ART_URL: &str =
    "https://storage.googleapis.com/freerealmsexpanded-assets/fill/v1/assetpack000_000.pack";
const ART_SHA256: &str = "634e7fe241acc0c5bc72f5b66fee6f901d39cc32dba469db9e9a5a055ef78c78";
const ART_SIZE: u64 = 22_313_291;
const ART_PACK: &str = "assets/assetpack000_000.pack";
const ART_ROWS: &str = include_str!("../assets/card-art-rows.txt");

/// What `TcgServerAddress` becomes.
pub const SET_ADDRESS: &str = "file://./tcg/";
const SET_MANIFEST: &str = "tcg/AssetsTcg_manifest.txt";
const SET_CRC: &str = "tcg/manifest.crc";
/// The client's own path for this name: `%03u/%s`.
const SET_LIST: &str = "tcg/968/TradingCardAssets.txt.z";
const LIST_ROW: &[u8] = b"TradingCardAssets.txt.z,";
const LIST: &[u8] = b"\r\n";
/// `LIST` as a `.z` file: magic, its length, and its zlib stream.
const LIST_FILE: [u8; 18] = [
    0xa1, 0xb2, 0xc3, 0xd4, 0, 0, 0, 2, 0x78, 0x9c, 0xe3, 0xe5, 2, 0, 0, 0x26, 0, 0x18,
];

/// Fetches the card art pack, checked against the hash above; does nothing once it is in place.
/// A failure only leaves the plain card backs: `install` adds the rows when the pack is there.
pub fn fetch_art(client: &Path, cancel: &AtomicBool) -> Result<()> {
    if art_installed(client) || !client.join(PACK).is_file() {
        return Ok(());
    }
    let body = patch::fetch_bytes(&Url::parse(ART_URL)?, ART_SIZE, cancel)?;
    if body.len() as u64 != ART_SIZE || patch::sha256(&body)? != ART_SHA256 {
        return Err("The card art failed its check.".into());
    }
    write(client, ART_PACK, &body)
}

/// `write` replaces whole files, so the right size is this version, whole.
fn art_installed(client: &Path) -> bool {
    fs::metadata(client.join(ART_PACK)).is_ok_and(|file| file.len() == ART_SIZE)
}

/// Prepares the card window, if this client has one. The DLL goes last: until then the client
/// behaves as it did.
pub fn install(client: &Path) -> Result<()> {
    if !client.join(DLL).is_file() || !client.join(PACK).is_file() {
        return Ok(());
    }
    local_set(client)?;
    let held = pack_rows(&client.join(PACK)).map_err(|e| e.to_string())?;
    if !client.join(STOCK).is_file() {
        fs::copy(client.join(MANIFEST), client.join(STOCK)).map_err(|e| e.to_string())?;
    }
    let manifest = fs::read(client.join(STOCK)).map_err(|e| e.to_string())?;
    let mut kept = kept_rows(&manifest, &held);
    if kept.is_empty() {
        return Err("The card art does not match its pack.".into());
    }
    if art_installed(client) {
        kept.extend(ART_ROWS.replace('\n', "\r\n").into_bytes());
    }
    write_changed(client, MANIFEST, &kept)?;
    write_changed(client, CRC, &crc_file(&kept))?;
    let dll = fs::read(client.join(DLL)).map_err(|e| e.to_string())?;
    if let Some(patched) = local_address(&dll) {
        write(client, DLL, &patched)?;
    }
    Ok(())
}

/// The card set's manifest row for the empty list, its crc file and the list.
fn local_set(client: &Path) -> Result<()> {
    let manifest = fs::read(client.join(SET_MANIFEST)).map_err(|e| e.to_string())?;
    let body = with_empty_list(&manifest).ok_or("The card set has no file list.")?;
    write_changed(client, SET_LIST, &LIST_FILE)?;
    write_changed(client, SET_MANIFEST, &body)?;
    write_changed(client, SET_CRC, &crc_file(&body))
}

/// The manifest with the list's row describing `LIST_FILE`: crc of the text, size of the stream.
fn with_empty_list(manifest: &[u8]) -> Option<Vec<u8>> {
    let mut found = false;
    let mut out = Vec::with_capacity(manifest.len());
    for row in manifest.split_inclusive(|byte| *byte == b'\n') {
        if row.starts_with(LIST_ROW) {
            found = true;
            out.extend(LIST_ROW);
            out.extend(format!("{},{}", crc32(LIST), LIST_FILE.len() - 8).into_bytes());
            out.extend(&row[row.trim_ascii_end().len()..]);
        } else {
            out.extend(row);
        }
    }
    found.then_some(out)
}

/// `manifest.crc`: `crc,length` of the manifest beside it.
fn crc_file(manifest: &[u8]) -> Vec<u8> {
    format!("{},{}\r\n", crc32(manifest), manifest.len()).into_bytes()
}

fn write_changed(client: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    if fs::read(client.join(relative)).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    write(client, relative, bytes)
}

/// A whole-file replace: a crash can leave the old file or the new one, never half of either.
fn write(client: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    if let Some(folder) = client.join(relative).parent() {
        fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    let target = safe_target(client, relative)?;
    let part = client.join(format!("{relative}.osfr-part"));
    let mut file = File::create(&part).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&part, &target).map_err(|e| e.to_string())
}

/// The DLL with the address replaced in place, padded so nothing moves; `None` if it isn't there.
fn local_address(dll: &[u8]) -> Option<Vec<u8>> {
    let at = dll.windows(DEAD.len()).position(|bytes| bytes == DEAD)?;
    let mut patched = dll.to_vec();
    patched[at..at + DEAD.len()].fill(0);
    patched[at..at + LOCAL.len()].copy_from_slice(LOCAL);
    Some(patched)
}

/// The manifest's `name,crc,size` rows that the pack holds at that crc, byte for byte.
fn kept_rows(manifest: &[u8], held: &HashMap<Vec<u8>, u32>) -> Vec<u8> {
    let mut kept = Vec::with_capacity(manifest.len());
    for row in manifest.split_inclusive(|byte| *byte == b'\n') {
        let mut fields = row.split(|byte| *byte == b',');
        let (Some(name), Some(crc)) = (fields.next(), fields.next()) else {
            continue;
        };
        let crc = std::str::from_utf8(crc).ok().and_then(|crc| crc.parse().ok());
        if crc.is_some() && held.get(name).copied() == crc {
            kept.extend(row);
        }
    }
    kept
}

/// A pack's table: chunks of (next chunk, count), then (name length, name, offset, size, crc), all
/// big-endian. Only the table is read.
fn pack_rows(path: &Path) -> std::io::Result<HashMap<Vec<u8>, u32>> {
    let mut pack = BufReader::new(File::open(path)?);
    let word = |pack: &mut BufReader<File>| -> std::io::Result<u32> {
        let mut bytes = [0; 4];
        pack.read_exact(&mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    };
    let bad = || std::io::Error::other("damaged pack table");
    let (mut rows, mut at) = (HashMap::new(), 0);
    loop {
        pack.seek(SeekFrom::Start(u64::from(at)))?;
        let (next, count) = (word(&mut pack)?, word(&mut pack)?);
        if count > MAX_ROWS || rows.len() as u32 + count > MAX_ROWS {
            return Err(bad());
        }
        for _ in 0..count {
            let size = word(&mut pack)?;
            if size > 4096 {
                return Err(bad());
            }
            let mut name = vec![0; size as usize];
            pack.read_exact(&mut name)?;
            let (_offset, _length, crc) = (word(&mut pack)?, word(&mut pack)?, word(&mut pack)?);
            rows.insert(name, crc);
        }
        if next == 0 {
            return Ok(rows);
        }
        if next <= at {
            return Err(bad());
        }
        at = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Client(PathBuf);

    impl Drop for Client {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pack(chunks: &[&[(&str, u32)]]) -> Vec<u8> {
        let mut out = Vec::new();
        for (index, rows) in chunks.iter().enumerate() {
            let mut table = Vec::new();
            for (name, crc) in *rows {
                table.extend((name.len() as u32).to_be_bytes());
                table.extend(name.as_bytes());
                table.extend([0; 8]);
                table.extend(crc.to_be_bytes());
            }
            let next = if index + 1 == chunks.len() { 0 } else { out.len() + 8 + table.len() };
            out.extend((next as u32).to_be_bytes());
            out.extend((rows.len() as u32).to_be_bytes());
            out.extend(table);
        }
        out
    }

    fn client(name: &str) -> Client {
        let root = std::env::temp_dir().join(format!("card-art-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tcg")).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        let dll = [b"before".as_slice(), DEAD, b"after"].concat();
        fs::write(root.join(DLL), dll).unwrap();
        let chunks: &[&[(&str, u32)]] = &[&[("a.png", 1), ("b.png", 2)], &[("c.png", 3)]];
        fs::write(root.join(PACK), pack(chunks)).unwrap();
        let rows = "a.png,1,10\r\nb.png,9,10\r\nc.png,3,10\r\nmissing.png,4,10\r\n";
        fs::write(root.join(MANIFEST), rows).unwrap();
        let set = "cards.rcc.z,5,10\r\nTradingCardAssets.txt.z,4166378948,370\r\nindex.z,6,10\r\n";
        fs::write(root.join(SET_MANIFEST), set).unwrap();
        Client(root.canonicalize().unwrap())
    }

    /// The art pack's rows join the manifest only once the pack is really there.
    #[test]
    fn the_art_rows_arrive_with_the_art_pack() {
        let client = client("art");
        let first = ART_ROWS.lines().next().unwrap();
        install(&client.0).unwrap();
        let manifest = fs::read_to_string(client.0.join(MANIFEST)).unwrap();
        assert!(!manifest.contains(first), "no pack, no rows");
        File::create(client.0.join(ART_PACK)).unwrap().set_len(ART_SIZE).unwrap();
        install(&client.0).unwrap();
        let manifest = fs::read(client.0.join(MANIFEST)).unwrap();
        assert!(String::from_utf8_lossy(&manifest).contains(first));
        assert_eq!(fs::read(client.0.join(CRC)).unwrap(), crc_file(&manifest));
    }

    /// Downloads the real pack: `cargo test -- --ignored`.
    #[test]
    #[ignore = "fetches 22 MB from the bucket"]
    fn the_bucket_serves_the_pinned_art() {
        let url = Url::parse(ART_URL).unwrap();
        let body = patch::fetch_bytes(&url, ART_SIZE, &AtomicBool::new(false)).unwrap();
        assert_eq!(body.len() as u64, ART_SIZE);
        assert_eq!(patch::sha256(&body).unwrap(), ART_SHA256);
    }

    /// By hand, on a real client: `CARD_ART_CLIENT=<folder>`, then run the ignored tests.
    #[test]
    #[ignore = "changes the client named by CARD_ART_CLIENT"]
    fn a_real_client() {
        install(Path::new(&std::env::var("CARD_ART_CLIENT").unwrap())).unwrap();
    }

    /// The address is swapped in place, and only rows the pack holds at that crc stay.
    #[test]
    fn the_card_window_reads_its_art_locally() {
        let client = client("install");
        install(&client.0).unwrap();
        let dll = fs::read(client.0.join(DLL)).unwrap();
        let expected = [b"before".as_slice(), LOCAL, &[0; 49], b"after"].concat();
        assert_eq!(dll, expected);
        assert_eq!(dll.len(), 6 + DEAD.len() + 5, "nothing in the DLL moves");
        let manifest = fs::read(client.0.join(MANIFEST)).unwrap();
        assert_eq!(manifest, b"a.png,1,10\r\nc.png,3,10\r\n");
        let crc = format!("{},{}\r\n", crc32(&manifest), manifest.len());
        assert_eq!(fs::read(client.0.join(CRC)).unwrap(), crc.into_bytes());
    }

    /// The card set gets a real `.z` of the empty list, and a manifest that describes it.
    #[test]
    fn the_card_set_gets_an_empty_list() {
        let client = client("set");
        install(&client.0).unwrap();
        assert_eq!(fs::read(client.0.join(SET_LIST)).unwrap(), LIST_FILE);
        let manifest = fs::read(client.0.join(SET_MANIFEST)).unwrap();
        let row = format!("TradingCardAssets.txt.z,{},10\r\n", crc32(LIST));
        let expected = format!("cards.rcc.z,5,10\r\n{row}index.z,6,10\r\n");
        assert_eq!(manifest, expected.into_bytes());
        assert_eq!(fs::read(client.0.join(SET_CRC)).unwrap(), crc_file(&manifest));
    }

    /// A row trimmed today can come back: the stock manifest keeps it, and a later pack holds it.
    #[test]
    fn the_stock_manifest_keeps_every_row() {
        let client = client("stock");
        let shipped = fs::read(client.0.join(MANIFEST)).unwrap();
        install(&client.0).unwrap();
        assert_eq!(fs::read(client.0.join(STOCK)).unwrap(), shipped);
        let chunks: &[&[(&str, u32)]] = &[&[("a.png", 1), ("c.png", 3), ("missing.png", 4)]];
        fs::write(client.0.join(PACK), pack(chunks)).unwrap();
        install(&client.0).unwrap();
        let manifest = fs::read(client.0.join(MANIFEST)).unwrap();
        assert_eq!(manifest, b"a.png,1,10\r\nc.png,3,10\r\nmissing.png,4,10\r\n");
    }

    #[test]
    fn it_can_run_at_every_launch() {
        let client = client("again");
        install(&client.0).unwrap();
        let files = [DLL, MANIFEST, CRC, SET_MANIFEST, SET_CRC, SET_LIST];
        let first = files.map(|name| fs::read(client.0.join(name)).unwrap());
        install(&client.0).unwrap();
        let second = files.map(|name| fs::read(client.0.join(name)).unwrap());
        assert_eq!(first, second);
    }

    #[test]
    fn a_client_without_a_card_window_is_left_alone() {
        let client = client("none");
        fs::remove_file(client.0.join(PACK)).unwrap();
        let before = fs::read(client.0.join(DLL)).unwrap();
        install(&client.0).unwrap();
        assert_eq!(fs::read(client.0.join(DLL)).unwrap(), before);
        assert!(!client.0.join(CRC).exists());
    }

    /// A pack that matches nothing changes nothing: the DLL keeps its old address.
    #[test]
    fn a_mismatched_pack_is_refused_before_the_dll() {
        let client = client("mismatch");
        fs::write(client.0.join(MANIFEST), "other.png,1,10\r\n").unwrap();
        let before = fs::read(client.0.join(DLL)).unwrap();
        assert!(install(&client.0).is_err());
        assert_eq!(fs::read(client.0.join(DLL)).unwrap(), before);
    }

    #[test]
    fn a_table_that_loops_is_refused() {
        let client = client("loop");
        let mut bytes = pack(&[&[("a.png", 1)], &[("b.png", 2)]]);
        let second = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        let own = (second as u32).to_be_bytes();
        bytes[second..second + 4].copy_from_slice(&own);
        fs::write(client.0.join(PACK), bytes).unwrap();
        assert!(install(&client.0).is_err());
    }
}
