//! What a port needs from the user: files in place, or a ROM of the right version,
//! told by its SHA-1. ROM hashes are kept in cache/roms.json by path, size and time.
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha1::Digest;

use crate::index::{Need, Port};

#[derive(Clone, Debug, PartialEq)]
pub enum Have {
    /// Nothing to check on the card: the port's page says what to do.
    Unknown,
    Missing,
    /// In place (the path found).
    Found(String),
    /// A ROM of the game, but not the version the port is made for.
    WrongVersion(String),
}

const ROM_EXTS: [&str; 3] = ["z64", "v64", "n64"];

#[derive(Serialize, Deserialize, Default)]
pub struct RomCache {
    /// path -> (size, modified seconds, sha1 of the big-endian bytes, header title)
    roms: BTreeMap<String, (u64, u64, String, String)>,
}

pub struct Roms {
    cache_path: PathBuf,
    cache: RomCache,
    dirty: bool,
}

impl Roms {
    pub fn open(home: &Path) -> Roms {
        let cache_path = home.join("cache").join("roms.json");
        let cache = fs::read_to_string(&cache_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Roms {
            cache_path,
            cache,
            dirty: false,
        }
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(dir) = self.cache_path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_vec(&self.cache) {
            let _ = fs::write(&self.cache_path, text);
        }
        self.dirty = false;
    }

    /// The SHA-1 of the ROM at `path` and the title in its header, from the cache
    /// when the file is unchanged.
    pub fn sha1(&mut self, path: &Path) -> io::Result<(String, String)> {
        let meta = fs::metadata(path)?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let key = path.to_string_lossy().into_owned();
        if let Some((size, time, sha, title)) = self.cache.roms.get(&key) {
            if *size == meta.len() && *time == modified {
                return Ok((sha.clone(), title.clone()));
            }
        }
        let (sha, title) = n64_sha1(path)?;
        self.cache.roms.insert(
            key,
            (meta.len(), modified, sha.clone(), title.clone()),
        );
        self.dirty = true;
        Ok((sha, title))
    }
}

/// SHA-1 of an N64 ROM in big-endian (.z64) order, whatever order the dump is in:
/// told by the first word (80 37 12 40 big-endian, 37 80 40 12 byte-swapped,
/// 40 12 37 80 little-endian). With it, the title in the header (bytes 0x20..0x34).
pub fn n64_sha1(path: &Path) -> io::Result<(String, String)> {
    let mut f = fs::File::open(path)?;
    let mut h = sha1::Sha1::new();
    let mut buf = vec![0; 1 << 16];
    let mut order = None;
    let mut title = String::new();
    loop {
        let n = read_full(&mut f, &mut buf)?;
        if n == 0 {
            break;
        }
        let chunk = &mut buf[..n];
        let order = *order.get_or_insert(match chunk.get(..4) {
            Some([0x37, 0x80, 0x40, 0x12]) => 2,
            Some([0x40, 0x12, 0x37, 0x80]) => 4,
            _ => 0,
        });
        match order {
            2 => chunk.chunks_exact_mut(2).for_each(|w| w.swap(0, 1)),
            4 => chunk.chunks_exact_mut(4).for_each(|w| w.reverse()),
            _ => {}
        }
        if title.is_empty() && chunk.len() >= 0x34 {
            title = String::from_utf8_lossy(&chunk[0x20..0x34]).trim().to_string();
        }
        h.update(chunk);
    }
    Ok((crate::fetch::hex(&h.finalize()), title))
}

fn read_full(f: &mut fs::File, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// ROM files in `dir` and one level below.
fn rom_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(|e| e.ok()) {
        let path = e.path();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() && depth > 0 {
            rom_files(&path, depth - 1, out);
        } else if kind.is_file()
            && path
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| ROM_EXTS.contains(&x.to_ascii_lowercase().as_str()))
        {
            out.push(path);
        }
    }
}

/// What the card has of each of `port`'s needs. Hashing a ROM takes a while on the
/// device: `hashing` is told each file before it is read.
pub fn check(card: &Path, port: &Port, roms: &mut Roms, hashing: &mut dyn FnMut(&Path)) -> Vec<Have> {
    port.needs
        .iter()
        .map(|need| check_one(card, need, roms, hashing))
        .collect()
}

fn check_one(card: &Path, need: &Need, roms: &mut Roms, hashing: &mut dyn FnMut(&Path)) -> Have {
    if !need.sha1.is_empty() {
        let mut files = Vec::new();
        for dir in &need.search {
            rom_files(&card.join(dir), 1, &mut files);
        }
        files.sort();
        files.dedup();
        let mut wrong = None;
        for f in files {
            hashing(&f);
            let Ok((sha, title)) = roms.sha1(&f) else {
                continue;
            };
            let shown = f
                .strip_prefix(card)
                .unwrap_or(&f)
                .to_string_lossy()
                .into_owned();
            if sha.eq_ignore_ascii_case(&need.sha1) {
                roms.save();
                return Have::Found(shown);
            }
            // Another game's ROM is not worth a word; the same game's in another
            // version (or region) is.
            if !need.title.is_empty() && plain(&title).starts_with(&plain(&need.title)) {
                wrong.get_or_insert(shown);
            }
        }
        roms.save();
        return match wrong {
            Some(w) => Have::WrongVersion(w),
            None => Have::Missing,
        };
    }
    if need.check.is_empty() {
        return Have::Unknown;
    }
    match need.check.iter().find_map(|p| find_like_import(card, p)) {
        Some(p) => Have::Found(p),
        None => Have::Missing,
    }
}

/// `path` on the card, or a file of its name (in any case) up to two folders down
/// from its folder: where Onion's ~Import ports accepts a game's data file.
fn find_like_import(card: &Path, path: &str) -> Option<String> {
    if card.join(path).exists() {
        return Some(path.to_string());
    }
    let (dir, name) = path.rsplit_once('/')?;
    fn walk(card: &Path, dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
        let mut subdirs = Vec::new();
        for e in fs::read_dir(dir).ok()?.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                subdirs.push(p);
            } else if e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
                return Some(p);
            }
        }
        if depth > 1 {
            subdirs.sort();
            return subdirs.iter().find_map(|d| walk(card, d, name, depth - 1));
        }
        None
    }
    let found = walk(card, &card.join(dir), name, 2)?;
    Some(found.strip_prefix(card).ok()?.to_string_lossy().into_owned())
}

/// Letters and digits only, in capitals: "Majora's Mask" -> "MAJORASMASK".
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}
