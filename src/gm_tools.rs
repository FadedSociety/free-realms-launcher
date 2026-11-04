//! GM tools (owner 2026-10-31): the in-game GM panel and the GM Tools / Redeem Code hub buttons,
//! installed into this PC's own client. The panel is Sanctuary PR #163's Referee Tools panel
//! (BubbaJoeX, AGPL-3.0; sources and credit in `assets/gm/`).
//!
//! The UI movies are the player's own: the stock `chatWindowCtrl.gfx` and `gamedock.gfx` are read
//! from the client's packs and our compiled script is added as one more DoAction tag after the
//! frame's stock script (AS2 runs a frame's DoActions in order, sharing its timeline), so no game
//! code is copied or recompiled. The movies go into one extra `Assets_NNN.pack` under new names,
//! the window definitions point at them, and `Assets_manifest.txt` lists them: the client only
//! loads a UI movie from a pack when the manifest's crc matches (the asset Responder then serves
//! that manifest's crc, so the client keeps it).
use crate::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};

const PANEL: &[u8] = include_bytes!("../assets/gm/gm_panel.doaction");
const HUB: &[u8] = include_bytes!("../assets/gm/gm_hub_button.doaction");
/// Records which pack holds the tools, so reinstalling rewrites the same one.
const MARKER: &str = "frx_gm_tools.txt";
const BACKUP: &str = ".frx-backup";
const ENVELOPE: u32 = 0xA1B2C3D4;
const ALIGNMENT: usize = 8192;
/// (R) Bounds an inflated UI movie; the stock chat window is ~160 KB.
const MOVIE_LIMIT: usize = 64 * 1024 * 1024;
/// The admin GM wheel's words the stock locale lacks: the server's `player_gm_menu::LABELS`
/// (same ids, same order). Remove leaves them: text nothing names is harmless.
const LABELS: [(u32, &str); 16] = [
    (7_900_000, "GM"),
    (7_900_001, "Ban"),
    (7_900_002, "Summon"),
    (7_900_003, "Role"),
    (7_900_004, "10 minutes"),
    (7_900_005, "1 hour"),
    (7_900_006, "1 day"),
    (7_900_007, "Forever"),
    (7_900_008, "100"),
    (7_900_009, "1,000"),
    (7_900_010, "10,000"),
    (7_900_011, "100,000"),
    (7_900_012, "1,000,000"),
    (7_900_013, "Player"),
    (7_900_014, "Tester"),
    (7_900_015, "Moderator"),
];

struct Mod {
    stock: &'static str,
    published: &'static str,
    frame: u16,
    script: &'static [u8],
    window: &'static str,
    edits: &'static [(&'static str, &'static str)],
}

const MODS: [Mod; 2] = [
    Mod {
        stock: "chatWindowCtrl",
        published: "frxChatWindowCtrl",
        frame: 2,
        script: PANEL,
        window: "UI/UiModules/Main/wndChatLog.xml",
        // The client clips a movie to its window: the larger window lets the panel move anywhere,
        // and the chat itself stays drawn 1:1 at the bottom left (the PR's layout).
        edits: &[
            ("UI\\chatWindowCtrl.swf", "UI\\frxChatWindowCtrl.swf"),
            (
                "Width=\"550\" Height=\"345\"",
                "Width=\"2000\" Height=\"1045\"",
            ),
        ],
    },
    Mod {
        stock: "gamedock",
        published: "frxGameDock",
        frame: 1,
        script: HUB,
        window: "UI/UiModules/Main/wndGameDock.xml",
        edits: &[("UI\\gamedock.swf", "UI\\frxGameDock.swf")],
    },
];

pub fn installed(client: &Path) -> bool {
    client.join(MARKER).is_file()
}

/// Build and install the tools from this client's own movies; safe to run again.
pub fn install(client: &Path) -> Result<()> {
    let mut assets = Vec::new();
    let mut lines = Vec::new();
    for m in &MODS {
        let movie = pack_file(client, &format!("{}.gfx", m.stock))?;
        let movie = add_script(&movie, m.frame, m.script)?;
        let list = pack_file(client, &format!("{}.lst", m.stock))?;
        let list = replace_bytes(
            &list,
            format!("{}.gfx", m.stock).as_bytes(),
            format!("{}.gfx", m.published).as_bytes(),
        );
        // Manifest rows as the stock ones: `name,crc32,size`; a `.lst` is published as `.lst.z`
        // with the crc of the plain list and the size of its A1B2C3D4 envelope.
        lines.push(format!(
            "{}.gfx,{},{}",
            m.published,
            crc32(&movie),
            movie.len()
        ));
        lines.push(format!(
            "{}.lst.z,{},{}",
            m.published,
            crc32(&list),
            envelope(&list).len()
        ));
        assets.push((format!("{}.gfx", m.published), movie));
        assets.push((format!("{}.lst", m.published), list));
    }
    let pack = match fs::read_to_string(client.join(MARKER)) {
        Ok(name) if valid_pack_name(name.trim()) => client.join(name.trim()),
        _ => next_pack(client),
    };
    fs::write(&pack, write_pack(&assets)).map_err(|e| e.to_string())?;
    fs::write(
        client.join(MARKER),
        pack.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    for m in &MODS {
        patch_window(client, m)?;
    }
    let labels: Vec<_> = LABELS
        .iter()
        .map(|&(hash, text)| crate::client_text::Row {
            hash,
            tag: "ugdt".into(),
            text: text.into(),
        })
        .collect();
    crate::client_text::add_rows(client, &labels).map_err(|e| format!("GM wheel labels: {e}"))?;
    set_manifest_rows(client, &lines)
}

/// Undo `install`: the stock windows come back, the pack and the manifest rows go.
pub fn remove(client: &Path) -> Result<()> {
    for m in &MODS {
        let path = client.join(m.window);
        let backup = backup_of(&path);
        if backup.is_file() {
            fs::copy(&backup, &path).map_err(|e| e.to_string())?;
            fs::remove_file(&backup).map_err(|e| e.to_string())?;
        }
    }
    set_manifest_rows(client, &[])?;
    if let Ok(name) = fs::read_to_string(client.join(MARKER)) {
        if valid_pack_name(name.trim()) {
            let _ = fs::remove_file(client.join(name.trim()));
        }
    }
    let _ = fs::remove_file(client.join(MARKER));
    Ok(())
}

fn backup_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP);
    path.with_file_name(name)
}

/// Always edits the stock text (kept as a backup), so a reinstall or an upgrade is idempotent.
fn patch_window(client: &Path, m: &Mod) -> Result<()> {
    let path = client.join(m.window);
    let backup = backup_of(&path);
    if !backup.is_file() {
        fs::copy(&path, &backup).map_err(|e| e.to_string())?;
    }
    let mut text = fs::read_to_string(&backup).map_err(|e| e.to_string())?;
    for (stock, ours) in m.edits {
        if !text.contains(stock) {
            return Err(format!(
                "{} does not look like the stock window ({stock} is missing); GM tools were not installed.",
                m.window
            ));
        }
        text = text.replace(stock, ours);
    }
    fs::write(&path, text).map_err(|e| e.to_string())
}

/// Our rows replace any earlier ones; every other row stays byte for byte.
fn set_manifest_rows(client: &Path, rows: &[String]) -> Result<()> {
    let path = client.join("Assets_manifest.txt");
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut out: String = text
        .split_inclusive('\n')
        .filter(|line| {
            !MODS
                .iter()
                .any(|m| line.starts_with(&format!("{}.", m.published)))
        })
        .collect();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str("\r\n");
    }
    for row in rows {
        out.push_str(row);
        out.push_str("\r\n");
    }
    fs::write(&path, out).map_err(|e| e.to_string())
}

fn valid_pack_name(name: &str) -> bool {
    name.len() == "Assets_000.pack".len()
        && name.starts_with("Assets_")
        && name.ends_with(".pack")
        && name[7..10].bytes().all(|b| b.is_ascii_digit())
}

fn next_pack(client: &Path) -> PathBuf {
    (0..1000)
        .map(|n| client.join(format!("Assets_{n:03}.pack")))
        .find(|path| !path.exists())
        .unwrap_or_else(|| client.join("Assets_999.pack"))
}

fn pack_file(client: &Path, name: &str) -> Result<Vec<u8>> {
    crate::character_select::pack_entry(client, name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("The client has no {name}; download the client again in Settings."))
}

/// A one-chunk pack: the index, padded to 8 KiB, then each file.
fn write_pack(assets: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    write_pack_to(&mut out, assets).expect("writing to a Vec cannot fail");
    out
}

/// One pack chunk in the client's layout: the index (big-endian next chunk 0, count, then per
/// entry name length, name, offset, length, crc), zero-padded to [`ALIGNMENT`], then the bodies.
pub(crate) fn write_pack_to(
    out: &mut impl std::io::Write,
    assets: &[(String, Vec<u8>)],
) -> std::io::Result<()> {
    let index_size = 8 + assets.iter().map(|(n, _)| 4 + n.len() + 12).sum::<usize>();
    let mut offset = index_size.max(ALIGNMENT);
    let mut index = Vec::with_capacity(offset);
    index.extend(0u32.to_be_bytes());
    index.extend((assets.len() as u32).to_be_bytes());
    for (name, data) in assets {
        index.extend((name.len() as u32).to_be_bytes());
        index.extend(name.as_bytes());
        index.extend((offset as u32).to_be_bytes());
        index.extend((data.len() as u32).to_be_bytes());
        index.extend(crc32(data).to_be_bytes());
        offset += data.len();
    }
    index.resize(index_size.max(ALIGNMENT), 0);
    out.write_all(&index)?;
    for (_, data) in assets {
        out.write_all(data)?;
    }
    Ok(())
}

/// Add `script` (complete DoAction tag records) after the stock DoAction of `frame`, before that
/// frame's ShowFrame. GFX movies are SWF with a `GFX`/`CFX` signature (CFX: zlib after 8 bytes).
fn add_script(movie: &[u8], frame: u16, script: &[u8]) -> Result<Vec<u8>> {
    let bad =
        || "A UI movie in the client could not be read; download the client again.".to_owned();
    let (signature, version) = (
        movie.get(..3).ok_or_else(bad)?,
        *movie.get(3).ok_or_else(bad)?,
    );
    let body = match signature {
        b"CFX" => miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&movie[8..], MOVIE_LIMIT)
            .map_err(|_| bad())?,
        b"GFX" => movie[8..].to_vec(),
        _ => return Err(bad()),
    };
    let nbits = (*body.first().ok_or_else(bad)? >> 3) as usize;
    let mut at = (5 + 4 * nbits).div_ceil(8) + 4;
    let mut current = 1u16;
    let insert = loop {
        let head = u16::from_le_bytes(body.get(at..at + 2).ok_or_else(bad)?.try_into().unwrap());
        let (code, short) = (head >> 6, (head & 0x3F) as usize);
        let (header, length) = if short == 0x3F {
            let long = u32::from_le_bytes(
                body.get(at + 2..at + 6)
                    .ok_or_else(bad)?
                    .try_into()
                    .unwrap(),
            );
            (6, long as usize)
        } else {
            (2, short)
        };
        if code == 1 && current == frame {
            break at;
        }
        if code == 0 {
            return Err(bad());
        }
        if code == 1 {
            current += 1;
        }
        at += header + length;
    };
    let mut patched = Vec::with_capacity(body.len() + script.len());
    patched.extend(&body[..insert]);
    patched.extend(script);
    patched.extend(&body[insert..]);
    let mut out = Vec::with_capacity(patched.len());
    out.extend(signature);
    out.push(version);
    out.extend(((patched.len() + 8) as u32).to_le_bytes());
    if signature == b"CFX" {
        out.extend(miniz_oxide::deflate::compress_to_vec_zlib(&patched, 9));
    } else {
        out.extend(patched);
    }
    Ok(out)
}

fn envelope(data: &[u8]) -> Vec<u8> {
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(data, 9);
    [
        &ENVELOPE.to_be_bytes()[..],
        &(data.len() as u32).to_be_bytes(),
        &packed,
    ]
    .concat()
}

fn replace_bytes(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    while at < data.len() {
        if data[at..].starts_with(from) {
            out.extend(to);
            at += from.len();
        } else {
            out.push(data[at]);
            at += 1;
        }
    }
    out
}

/// Standard CRC-32 (the manifest's and the pack index's).
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let table: [u32; 256] = std::array::from_fn(|n| {
        (0..8).fold(n as u32, |c, _| {
            (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg())
        })
    });
    !bytes.iter().fold(!0_u32, |c, &b| {
        table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-frame movie: frame 1 empty, frame 2 one DoAction then ShowFrame, then End.
    fn movie(compressed: bool) -> Vec<u8> {
        let tag = |code: u16, body: &[u8]| {
            let mut t = ((code << 6) | body.len() as u16).to_le_bytes().to_vec();
            t.extend(body);
            t
        };
        let mut body = vec![0x00, 0, 24, 2, 0]; // RECT nbits 0 (1 byte), rate, count 2
        body.extend(tag(1, &[]));
        body.extend(tag(12, &[0xAA, 0]));
        body.extend(tag(1, &[]));
        body.extend(tag(0, &[]));
        let mut out = if compressed {
            b"CFX\x09".to_vec()
        } else {
            b"GFX\x09".to_vec()
        };
        out.extend(((body.len() + 8) as u32).to_le_bytes());
        if compressed {
            out.extend(miniz_oxide::deflate::compress_to_vec_zlib(&body, 6));
            out
        } else {
            out.extend(body);
            out
        }
    }

    #[test]
    fn our_script_follows_the_frames_stock_script_and_the_header_length_grows() {
        let ours = [((12u16 << 6) | 2).to_le_bytes().as_slice(), &[0xBB, 0]].concat();
        for compressed in [false, true] {
            let patched = add_script(&movie(compressed), 2, &ours).unwrap();
            let body = if compressed {
                miniz_oxide::inflate::decompress_to_vec_zlib(&patched[8..]).unwrap()
            } else {
                patched[8..].to_vec()
            };
            let stock = [((12u16 << 6) | 2).to_le_bytes().as_slice(), &[0xAA, 0]].concat();
            let both = [stock.as_slice(), &ours].concat();
            let at = body.windows(both.len()).position(|w| w == both.as_slice());
            assert!(at.is_some(), "stock DoAction, then ours");
            assert_eq!(
                &body[at.unwrap() + both.len()..][..2],
                &(1u16 << 6).to_le_bytes(),
                "then ShowFrame"
            );
            let length = u32::from_le_bytes(patched[4..8].try_into().unwrap()) as usize;
            assert_eq!(length, body.len() + 8);
        }
        assert!(
            add_script(&movie(false), 3, &ours).is_err(),
            "no third frame"
        );
    }

    /// Dev check against a real client copy (OSFR_GM_CLIENT): install, then remove.
    #[test]
    #[ignore = "needs a client copy"]
    fn installs_into_and_removes_from_a_real_client_copy() {
        let client = PathBuf::from(std::env::var_os("OSFR_GM_CLIENT").unwrap());
        let window = fs::read_to_string(client.join(MODS[0].window)).unwrap();
        let manifest = fs::read(client.join("Assets_manifest.txt")).unwrap();
        install(&client).unwrap();
        install(&client).unwrap();
        assert!(installed(&client));
        let movie = pack_file(&client, "frxChatWindowCtrl.gfx").unwrap();
        let inflated = miniz_oxide::inflate::decompress_to_vec_zlib(&movie[8..]).unwrap();
        assert!(inflated.windows(64).any(|w| w == &PANEL[..64]));
        assert!(
            fs::read_to_string(client.join(MODS[0].window))
                .unwrap()
                .contains("frxChatWindowCtrl.swf")
        );
        let rows = fs::read_to_string(client.join("Assets_manifest.txt")).unwrap();
        assert_eq!(
            rows.matches("frx").count(),
            4,
            "one row per published file, not doubled"
        );
        let locale = fs::read_to_string(client.join("locale/en_us_data.dat")).unwrap();
        assert_eq!(
            locale.matches("7900015\tugdt\tModerator").count(),
            1,
            "labels added once"
        );
        fs::write(client.join("frx-check.gfx"), &movie).unwrap();
        remove(&client).unwrap();
        assert_eq!(
            fs::read_to_string(client.join(MODS[0].window)).unwrap(),
            window
        );
        assert_eq!(
            fs::read(client.join("Assets_manifest.txt")).unwrap(),
            manifest
        );
        assert!(!installed(&client));
    }

    #[test]
    fn a_written_pack_is_read_back_and_manifest_rows_replace_ours_only() {
        let assets = vec![
            ("frxGameDock.gfx".to_owned(), vec![1, 2, 3]),
            (
                "frxGameDock.lst".to_owned(),
                b"#CRC64\nUI\\frxGameDock.gfx".to_vec(),
            ),
        ];
        let dir = std::env::temp_dir().join(format!("osfr-gm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Assets_007.pack"), write_pack(&assets)).unwrap();
        assert_eq!(pack_file(&dir, "frxGameDock.gfx").unwrap(), [1u8, 2, 3]);
        assert!(pack_file(&dir, "gamedock.gfx").is_err());
        assert_eq!(&envelope(b"abc")[..4], &ENVELOPE.to_be_bytes());
        fs::write(
            dir.join("Assets_manifest.txt"),
            "a.gfx,1,2\r\nfrxGameDock.gfx,9,9\r\nb.lst.z,3,4\r\n",
        )
        .unwrap();
        set_manifest_rows(&dir, &["frxGameDock.gfx,5,6".into()]).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("Assets_manifest.txt")).unwrap(),
            "a.gfx,1,2\r\nb.lst.z,3,4\r\nfrxGameDock.gfx,5,6\r\n"
        );
        set_manifest_rows(&dir, &[]).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("Assets_manifest.txt")).unwrap(),
            "a.gfx,1,2\r\nb.lst.z,3,4\r\n"
        );
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
