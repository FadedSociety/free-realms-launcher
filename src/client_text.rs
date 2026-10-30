//! Text the server shows that the stock client locale lacks (L073).
//!
//! The local server release carries `content/locale-additions.json`: rows of (hash, tag, text),
//! today the opening's reconstructed objective labels, which the stock journal shows as
//! "<STRING 0 NOT FOUND>". Before launch the launcher adds any missing row to the player's own
//! `locale/en_us_data.dat` and `.dir`, written the way the community locale tool writes them
//! (FreeRealmsLocaleTools, MIT): `.dat` rows in hash order after a UTF-8 BOM, CRLF lines of
//! `hash\ttag\ttext`; `.dir` header lines with `Count` and `MD5Checksum` (of the whole `.dat`)
//! kept in step, then `hash\toffset\tlength\tflag` per row. Rows already present are never changed.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

const BOM: &[u8] = b"\xef\xbb\xbf";
const LOCALE_LIMIT: u64 = 64 << 20;
const MAX_ROWS: usize = 10_000;

#[derive(Deserialize)]
struct Additions {
    schema_version: u32,
    locale: String,
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct Row {
    hash: u32,
    tag: String,
    text: String,
}

/// Add the rows of `additions` (a release's `locale-additions.json`) that `client`'s en_US locale
/// lacks. No file, or nothing missing, writes nothing. Returns how many rows were added.
pub fn ensure_rows(client: &Path, additions: &Path) -> io::Result<usize> {
    if !additions.try_exists()? {
        return Ok(0);
    }
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    let additions: Additions = serde_json::from_slice(&read_bounded(additions)?)
        .map_err(|e| invalid(&format!("locale additions: {e}")))?;
    if additions.schema_version != 1
        || additions.locale != "en_US"
        || additions.rows.len() > MAX_ROWS
    {
        return Err(invalid("locale additions: unsupported schema or locale"));
    }
    for row in &additions.rows {
        let clean = |s: &str| !s.contains(['\t', '\r', '\n']);
        if row.tag.len() != 4
            || !row.tag.bytes().all(|b| b.is_ascii_lowercase())
            || !clean(&row.text)
        {
            return Err(invalid("locale additions: a row is not one plain line"));
        }
    }
    let (dat_path, dir_path) = paths(client);
    let (dat, dir) = (read_bounded(&dat_path)?, read_bounded(&dir_path)?);
    let (merged, added) = merge(&dat, &dir, &additions.rows)?;
    if added == 0 {
        return Ok(0);
    }
    let (new_dat, new_dir) = merged;
    // .dat first: an interrupted write leaves the old .dir naming offsets that still parse.
    replace(&dat_path, &new_dat)?;
    replace(&dir_path, &new_dir)?;
    Ok(added)
}

fn paths(client: &Path) -> (PathBuf, PathBuf) {
    let locale = client.join("locale");
    (locale.join("en_us_data.dat"), locale.join("en_us_data.dir"))
}

/// The new `.dat` and `.dir`, and how many rows were added.
fn merge(dat: &[u8], dir: &[u8], rows: &[Row]) -> io::Result<((Vec<u8>, Vec<u8>), usize)> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    if !dat.starts_with(BOM) {
        return Err(invalid("locale .dat has no UTF-8 BOM"));
    }
    // Rows are read through the .dir (text may hold line breaks): hash, offset, length, flag.
    let mut header = Vec::new();
    let mut lines: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    let mut flags = BTreeMap::new();
    for line in dir.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.starts_with(b"##") {
            header.push(line.to_vec());
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&[u8]> = line.split(|b| *b == b'\t').collect();
        let number =
            |i: usize| -> Option<usize> { std::str::from_utf8(fields.get(i)?).ok()?.parse().ok() };
        let (Some(hash), Some(offset), Some(length), Some(flag)) =
            (key(line), number(1), number(2), fields.get(3))
        else {
            return Err(invalid("locale .dir row is not hash, offset, length, flag"));
        };
        let row = offset
            .checked_add(length)
            .and_then(|end| dat.get(offset..end))
            .filter(|row| key(row) == Some(hash))
            .ok_or_else(|| invalid("locale .dir row does not point at its .dat row"))?;
        if lines.insert(hash, row.to_vec()).is_some() {
            return Err(invalid("locale .dir repeats a hash"));
        }
        flags.insert(hash, flag.to_vec());
    }
    let mut added = 0;
    for row in rows {
        if !lines.contains_key(&row.hash) {
            let line = format!("{}\t{}\t{}", row.hash, row.tag, row.text).into_bytes();
            lines.insert(row.hash, line);
            added += 1;
        }
    }
    if added == 0 {
        return Ok(((Vec::new(), Vec::new()), 0));
    }
    let mut new_dat = BOM.to_vec();
    let mut index = Vec::new();
    for (hash, line) in &lines {
        let offset = new_dat.len();
        new_dat.extend(line);
        new_dat.extend(b"\r\n");
        let flag = flags.get(hash).map_or(&b"d"[..], Vec::as_slice);
        index.push((
            format!("{hash}\t{offset}\t{}\t", line.len()).into_bytes(),
            flag.to_vec(),
        ));
    }
    let md5 = crate::patch::md5_upper(&new_dat).map_err(io::Error::other)?;
    let mut new_dir = Vec::new();
    for line in header {
        let line = if line.starts_with(b"## Count:") {
            format!("## Count:\t{}", lines.len()).into_bytes()
        } else if line.starts_with(b"## MD5Checksum:") {
            format!("## MD5Checksum:\t{md5}").into_bytes()
        } else {
            line
        };
        new_dir.extend(line);
        new_dir.extend(b"\r\n");
    }
    for (row, flag) in index {
        new_dir.extend(row);
        new_dir.extend(flag);
        new_dir.extend(b"\r\n");
    }
    Ok(((new_dat, new_dir), added))
}

fn key(line: &[u8]) -> Option<u32> {
    let field = line.split(|b| *b == b'\t').next()?;
    std::str::from_utf8(field).ok()?.parse().ok()
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    if fs::metadata(path)?.len() > LOCALE_LIMIT {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
    }
    fs::read(path)
}

fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = path.with_extension("text-tmp");
    fs::write(&staged, bytes)?;
    fs::rename(&staged, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(hash: u32, text: &str) -> Row {
        Row {
            hash,
            tag: "ucdt".into(),
            text: text.into(),
        }
    }

    fn locale(rows: &[(u32, &str)]) -> (Vec<u8>, Vec<u8>) {
        let mut dat = BOM.to_vec();
        let mut dir = b"## Count:\t2\r\n## Game:\tFRLM\r\n## MD5Checksum:\tX\r\n".to_vec();
        for (hash, text) in rows {
            let line = format!("{hash}\tugdt\t{text}");
            dir.extend(format!("{hash}\t{}\t{}\td\r\n", dat.len(), line.len()).into_bytes());
            dat.extend(line.into_bytes());
            dat.extend(b"\r\n");
        }
        (dat, dir)
    }

    #[test]
    fn missing_rows_go_in_hash_order_and_the_header_follows() {
        let (dat, dir) = locale(&[(10, "ten"), (30, "thirty")]);
        let ((dat, dir), added) =
            merge(&dat, &dir, &[row(20, "twenty"), row(10, "not ten")]).unwrap();
        assert_eq!(added, 1);
        assert_eq!(
            dat,
            b"\xef\xbb\xbf10\tugdt\tten\r\n20\tucdt\ttwenty\r\n30\tugdt\tthirty\r\n"
        );
        let md5 = crate::patch::md5_upper(&dat).unwrap();
        let expected = format!(
            "## Count:\t3\r\n## Game:\tFRLM\r\n## MD5Checksum:\t{md5}\r\n10\t3\t11\td\r\n20\t16\t14\td\r\n30\t32\t14\td\r\n"
        );
        assert_eq!(String::from_utf8(dir).unwrap(), expected);
    }

    #[test]
    fn nothing_missing_writes_nothing() {
        let (dat, dir) = locale(&[(10, "ten")]);
        assert_eq!(merge(&dat, &dir, &[row(10, "ten")]).unwrap().1, 0);
    }

    #[test]
    fn md5_is_the_standard_digest() {
        assert_eq!(
            crate::patch::md5_upper(b"abc").unwrap(),
            "900150983CD24FB0D6963F7D28E17F72"
        );
    }

    /// By hand against the lab files: `OSFR_STOCK_LOCALE=<client>/locale OSFR_LAB_LOCALE=<package>/locale
    /// OSFR_ADDITIONS=<data/release/locale-additions.json> cargo test -- --ignored community_tool`.
    /// Our write of the release rows into a stock locale is byte-identical to the community tool's
    /// (the overlay the native harness proved in the client).
    #[test]
    #[ignore]
    fn matches_the_community_tool_byte_for_byte() {
        let var = |name| PathBuf::from(std::env::var(name).unwrap());
        let read = |dir: PathBuf, name: &str| fs::read(dir.join(name)).unwrap();
        let additions: Additions =
            serde_json::from_slice(&fs::read(var("OSFR_ADDITIONS")).unwrap()).unwrap();
        let ((dat, dir), added) = merge(
            &read(var("OSFR_STOCK_LOCALE"), "en_us_data.dat"),
            &read(var("OSFR_STOCK_LOCALE"), "en_us_data.dir"),
            &additions.rows,
        )
        .unwrap();
        assert_eq!(added, additions.rows.len());
        assert!(
            dat == read(var("OSFR_LAB_LOCALE"), "en_us_data.dat"),
            ".dat differs"
        );
        assert!(
            dir == read(var("OSFR_LAB_LOCALE"), "en_us_data.dir"),
            ".dir differs"
        );
    }
}
