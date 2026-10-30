//! Character select lists names, not ids.
//!
//! The stock `characterSelectV2.gfx` (Assets_006.pack entry 6072) titles each row with the
//! character's id: `CharacterItemRenderer` pushes constant 26 (`_id`) where it means constant 29
//! (`_name`), so a player sees "1" and "0" instead of their name and "Add a Character". The lab
//! fix (`lab/references/character-select-title-v3`, native run 163518) changes only that operand.
//! This builds the same one-byte change from the player's own copy and writes it to `custom/`,
//! which the client opens before its packs. Nothing from the client is shipped.
use std::{
    fs::{self, File},
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const MOVIE: &str = "characterSelectV2.gfx";
const STOCK_SHA256: &str = "1aa6779ca0168a01dc25860d0ea4e94e5f8c2ccd01bc6781acc46847bbbbe176";
/// The title push in the inflated movie: ActionPush, register 1, constant8 26 (`_id`).
const TITLE_PUSH: [u8; 7] = [0x96, 0x04, 0x00, 0x04, 0x01, 0x08, 0x1a];
const TITLE_AT: usize = 103_809;
const NAME_CONSTANT: u8 = 0x1d;
const MOVIE_LIMIT: u32 = 1 << 20;

/// Write `custom/characterSelectV2.gfx` once. Leaves an existing file alone, and does nothing
/// (Ok(false)) for a client whose movie is not the known stock one.
pub fn ensure_names(client: &Path) -> io::Result<bool> {
    let target = client.join("custom").join(MOVIE);
    if target.exists() {
        return Ok(false);
    }
    let Some(stock) = pack_entry(client, MOVIE)? else {
        return Ok(false);
    };
    if !crate::patch::sha256(&stock).is_ok_and(|h| h == STOCK_SHA256) {
        return Ok(false);
    }
    let patched = flip_title(&stock)
        .ok_or_else(|| io::Error::other("the stock character select movie did not inflate"))?;
    fs::create_dir_all(client.join("custom"))?;
    let staged = target.with_extension("names-tmp");
    fs::write(&staged, patched)?;
    fs::rename(&staged, &target)?;
    Ok(true)
}

/// CFX = "CFX", version, declared length, zlib body. Only the title operand changes; the header
/// and every other inflated byte stay as the client shipped them.
fn flip_title(movie: &[u8]) -> Option<Vec<u8>> {
    let (header, body) = (movie.get(..8)?, movie.get(8..)?);
    if !header.starts_with(b"CFX") {
        return None;
    }
    let mut inflated =
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(body, MOVIE_LIMIT as usize).ok()?;
    if inflated.get(TITLE_AT + 1 - TITLE_PUSH.len()..=TITLE_AT)? != TITLE_PUSH {
        return None;
    }
    inflated[TITLE_AT] = NAME_CONSTANT;
    let mut out = header.to_vec();
    out.extend(miniz_oxide::deflate::compress_to_vec_zlib(&inflated, 9));
    Some(out)
}

/// A file from the client's `Assets_*.pack` chain: big-endian chunks of (next chunk, count), then
/// per entry name length, name, offset, length, crc.
fn pack_entry(client: &Path, wanted: &str) -> io::Result<Option<Vec<u8>>> {
    let mut packs: Vec<_> = fs::read_dir(client)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("Assets_") && name.ends_with(".pack"))
        })
        .collect();
    packs.sort();
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    for pack in packs {
        let mut file = BufReader::new(File::open(&pack)?);
        let mut chunk = 0u64;
        loop {
            file.seek(SeekFrom::Start(chunk))?;
            let (next, count) = (be(&mut file)?, be(&mut file)?);
            if count > 100_000 {
                return Err(invalid("pack chunk lists too many files"));
            }
            for _ in 0..count {
                let length = be(&mut file)? as usize;
                if length > 1024 {
                    return Err(invalid("pack entry name too long"));
                }
                let mut name = vec![0; length];
                file.read_exact(&mut name)?;
                let (offset, size, _crc) = (be(&mut file)?, be(&mut file)?, be(&mut file)?);
                if name == wanted.as_bytes() {
                    if size > MOVIE_LIMIT {
                        return Err(invalid("pack entry too large"));
                    }
                    file.seek(SeekFrom::Start(offset.into()))?;
                    let mut body = vec![0; size as usize];
                    file.read_exact(&mut body)?;
                    return Ok(Some(body));
                }
            }
            if next == 0 || u64::from(next) <= chunk {
                break;
            }
            chunk = next.into();
        }
    }
    Ok(None)
}

fn be(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".test-data")
            .join(format!("character-select-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// A synthetic movie: the title push at its real offset, filler elsewhere.
    fn movie(push: [u8; 7]) -> Vec<u8> {
        let mut inflated = vec![0x5a; TITLE_AT + 64];
        inflated[TITLE_AT + 1 - push.len()..=TITLE_AT].copy_from_slice(&push);
        let mut movie = b"CFX\x0a".to_vec();
        movie.extend(((inflated.len() + 8) as u32).to_le_bytes());
        movie.extend(miniz_oxide::deflate::compress_to_vec_zlib(&inflated, 6));
        movie
    }

    #[test]
    fn the_title_push_reads_the_name_and_nothing_else_changes() {
        let stock = movie(TITLE_PUSH);
        let patched = flip_title(&stock).unwrap();
        assert_eq!(patched[..8], stock[..8]);
        let before = miniz_oxide::inflate::decompress_to_vec_zlib(&stock[8..]).unwrap();
        let after = miniz_oxide::inflate::decompress_to_vec_zlib(&patched[8..]).unwrap();
        let changed: Vec<_> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        assert_eq!(changed, [TITLE_AT]);
        assert_eq!(after[TITLE_AT], NAME_CONSTANT);
    }

    #[test]
    fn an_unexpected_movie_is_left_alone() {
        let mut other = TITLE_PUSH;
        other[6] = 0x1b;
        assert!(flip_title(&movie(other)).is_none());
        assert!(flip_title(b"FWS\x0a....").is_none());
    }

    #[test]
    fn a_pack_entry_is_found_across_chunks() {
        let root = scratch("pack");
        let entry = |name: &[u8], offset: u32, size: u32| {
            let mut e = (name.len() as u32).to_be_bytes().to_vec();
            e.extend(name);
            e.extend(offset.to_be_bytes());
            e.extend(size.to_be_bytes());
            e.extend(0u32.to_be_bytes());
            e
        };
        // Chunk 0 at 0 lists a.txt and links to chunk 1 at 64; the body sits at 128.
        let mut pack = vec![0; 160];
        let first = [64u32.to_be_bytes(), 1u32.to_be_bytes()].concat();
        pack[..8].copy_from_slice(&first);
        let a = entry(b"a.txt", 120, 2);
        pack[8..8 + a.len()].copy_from_slice(&a);
        let second = [0u32.to_be_bytes(), 1u32.to_be_bytes()].concat();
        pack[64..72].copy_from_slice(&second);
        let b = entry(MOVIE.as_bytes(), 128, 5);
        pack[72..72 + b.len()].copy_from_slice(&b);
        pack[128..133].copy_from_slice(b"movie");
        fs::write(root.join("Assets_006.pack"), &pack).unwrap();
        fs::write(root.join("AssetsW_000.pack"), b"not a pack").unwrap();
        assert_eq!(pack_entry(&root, MOVIE).unwrap().unwrap(), b"movie");
        assert!(pack_entry(&root, "missing.gfx").unwrap().is_none());
        // Not the stock movie: nothing is written.
        assert!(!ensure_names(&root).unwrap());
        assert!(!root.join("custom").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    /// Run by hand against a real client copy: `OSFR_CLIENT=<folder> cargo test -- --ignored
    /// real_client`. Writes nothing; checks the stock movie is found and flips.
    #[test]
    #[ignore]
    fn real_client_movie_flips() {
        let client = PathBuf::from(std::env::var("OSFR_CLIENT").unwrap());
        let stock = pack_entry(&client, MOVIE).unwrap().unwrap();
        assert_eq!(crate::patch::sha256(&stock).unwrap(), STOCK_SHA256);
        assert!(flip_title(&stock).is_some());
    }
}
