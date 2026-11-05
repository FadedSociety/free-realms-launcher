//! Game files the community host lacks, from our own bucket (owner 2026-11-05). They are one
//! read-only asset pack, pinned here by hash, plus the manifest rows for its converted files; the
//! rest of the pack answers rows the manifest already has. The rows change the manifest's crc, so
//! the client gets it from the local Responder, as with the GM tools.
use crate::{
    Result,
    patch::{self, Url},
};
use std::{collections::HashSet, fs, path::Path, sync::atomic::AtomicBool};

const URL: &str =
    "https://storage.googleapis.com/freerealmsexpanded-assets/fill/v1/Assets_fill.pack";
const SHA256: &str = "6eb55285f6799db8988f3348b154da3b46398856c7b744b51dedac81ac59439c";
const SIZE: u64 = 108_828_811;
const ROWS: &str = include_str!("../assets/fill-rows.txt");
const MARKER: &str = "osfr-fill.txt";
const MANIFEST: &str = "Assets_manifest.txt";

/// True once this version of the pack is in place.
pub fn installed(client: &Path) -> bool {
    fs::read_to_string(client.join(MARKER)).is_ok_and(|text| text.starts_with(SHA256))
}

/// Fetch the pack and add its rows; does nothing when it is already in place. The pack is checked
/// against the size and hash above before it lands.
pub fn install(client: &Path, cancel: &AtomicBool) -> Result<()> {
    if installed(client) {
        return Ok(());
    }
    let url = Url::parse(URL)?;
    let body = patch::fetch_bytes(&url, SIZE, cancel)?;
    if body.len() as u64 != SIZE || patch::sha256(&body)? != SHA256 {
        return Err("The extra game files failed their check.".into());
    }
    let old = fs::read_to_string(client.join(MARKER)).unwrap_or_default();
    if let Some(pack) = old.lines().nth(1).filter(|name| valid_pack(name)) {
        let _ = fs::remove_file(client.join(pack));
    }
    let mut number = 0;
    while client.join(pack_name(number)).exists() {
        number += 1;
    }
    let pack = pack_name(number);
    let part = client.join(format!("{pack}.osfr-part"));
    fs::write(&part, &body).map_err(|e| e.to_string())?;
    fs::rename(&part, client.join(&pack)).map_err(|e| e.to_string())?;
    set_rows(client, ROWS)?;
    fs::write(client.join(MARKER), format!("{SHA256}\n{pack}\n")).map_err(|e| e.to_string())
}

fn pack_name(number: u32) -> String {
    format!("Assets_{number:03}.pack")
}

fn valid_pack(name: &str) -> bool {
    name.strip_prefix("Assets_")
        .and_then(|rest| rest.strip_suffix(".pack"))
        .is_some_and(|n| n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A row's asset name: its first field, without the wire's `.z`.
fn asset(line: &str) -> String {
    let name = line.split(',').next().unwrap_or_default().trim();
    name.strip_suffix(".z").unwrap_or(name).to_ascii_lowercase()
}

/// Our rows replace any row for the same asset; every other row stays byte for byte.
fn set_rows(client: &Path, rows: &str) -> Result<()> {
    let path = client.join(MANIFEST);
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let ours: HashSet<String> = rows.lines().map(asset).collect();
    let mut out: String = text
        .split_inclusive('\n')
        .filter(|line| !ours.contains(&asset(line)))
        .collect();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(rows);
    fs::write(&path, out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("osfr-fill-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rows_replace_the_same_asset_and_keep_the_rest() {
        let dir = folder("rows");
        let manifest = "keep.dds.z,1,2\nsg_x.DDS.z,3,4\nlast.txt,5,6";
        fs::write(dir.join(MANIFEST), manifest).unwrap();
        set_rows(&dir, "sg_x.dds,7,8\nnew.dma,9,10\n").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(MANIFEST)).unwrap(),
            "keep.dds.z,1,2\nlast.txt,5,6\nsg_x.dds,7,8\nnew.dma,9,10\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_marker_names_this_version_only() {
        let dir = folder("marker");
        assert!(!installed(&dir));
        fs::write(dir.join(MARKER), format!("{SHA256}\nAssets_014.pack\n")).unwrap();
        assert!(installed(&dir));
        fs::write(dir.join(MARKER), "an older hash\nAssets_014.pack\n").unwrap();
        assert!(!installed(&dir));
        let _ = fs::remove_dir_all(dir);
    }

    /// Downloads the real pack: `cargo test -- --ignored`.
    #[test]
    #[ignore = "fetches 109 MB from the bucket"]
    fn the_bucket_serves_the_pinned_pack() {
        let dir = folder("live");
        fs::write(dir.join(MANIFEST), "keep.dds.z,1,2\n").unwrap();
        fs::write(dir.join("Assets_000.pack"), b"held").unwrap();
        install(&dir, &AtomicBool::new(false)).unwrap();
        assert!(installed(&dir));
        assert_eq!(fs::metadata(dir.join("Assets_001.pack")).unwrap().len(), SIZE);
        let manifest = fs::read_to_string(dir.join(MANIFEST)).unwrap();
        assert!(manifest.starts_with("keep.dds.z,1,2\n") && manifest.ends_with(ROWS));
        install(&dir, &AtomicBool::new(false)).unwrap();
        assert!(!dir.join("Assets_002.pack").exists(), "once");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn only_a_numbered_pack_is_ever_removed() {
        assert!(valid_pack("Assets_014.pack"));
        assert!(!valid_pack("AssetsW_000.pack"));
        assert!(!valid_pack("..\\Assets_014.pack"));
    }

    #[test]
    fn the_shipped_rows_are_well_formed() {
        assert!(ROWS.lines().count() > 500);
        for line in ROWS.lines() {
            let fields: Vec<_> = line.split(',').collect();
            assert_eq!(fields.len(), 3, "{line}");
            assert!(fields[1].parse::<u32>().is_ok() && fields[2].parse::<u64>().is_ok(), "{line}");
        }
    }
}
