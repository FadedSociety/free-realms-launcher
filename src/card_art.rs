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
use crate::{Result, assets::crc32};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const DLL: &str = "tcg/FreeRealmsTCG.dll";
const PACK: &str = "assets/assetpack000W_000.pack";
const MANIFEST: &str = "assets/assetpack000_manifest.txt";
const CRC: &str = "assets/manifest.crc";
/// The address in the DLL and what replaces it, both with the end of the string.
const DEAD: &[u8] = b"http://fr.patch.station.sony.com/patch/freerealmstcg/live/assets/\0";
const LOCAL: &[u8] = b"file://./assets/\0";
/// More rows than the 9,756 of the art manifest; a pack past this is damaged.
const MAX_ROWS: u32 = 1 << 20;

/// What `TcgServerAddress` becomes. The slash matters: a local address is joined without one.
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

/// Prepares the card window, if this client has one. The DLL goes last: until then the client
/// behaves as it did.
pub fn install(client: &Path) -> Result<()> {
    if !client.join(DLL).is_file() || !client.join(PACK).is_file() {
        return Ok(());
    }
    local_set(client)?;
    let held = pack_rows(&client.join(PACK)).map_err(|e| e.to_string())?;
    let manifest = fs::read(client.join(MANIFEST)).map_err(|e| e.to_string())?;
    let kept = kept_rows(&manifest, &held);
    if kept.is_empty() {
        return Err("The card art does not match its pack.".into());
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
    let target = client.join(relative);
    if let Some(folder) = target.parent() {
        fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    let part = client.join(format!("{relative}.osfr-part"));
    fs::write(&part, bytes).map_err(|e| e.to_string())?;
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
        Client(root)
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
