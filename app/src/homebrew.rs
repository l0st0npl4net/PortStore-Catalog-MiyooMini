//! Homebrew from Homebrew Hub (hh.gbdev.io): Game Boy, Game Boy Color, Game Boy
//! Advance and NES games, demos, tools and music, listed in homebrew.json next to
//! the catalog (index/homebrew.py). A ROM is downloaded from its database's git
//! repository at the commit the list was made from, checked against its git blob id,
//! and goes straight into OnionOS's folder for its system (Roms/GBA, Roms/GBC,
//! Roms/GB, Roms/FC), with its screenshot as box art. What was put there is recorded
//! in installed/homebrew/<slug>.json, so removal takes exactly that.
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha1::Digest;

use crate::install::{move_file, Step};
use crate::{fetch, miyooports, needs, State, Store};

pub const FILE: &str = "homebrew.json";
/// Where the ROMs and their screenshots come from: nothing else is downloaded.
const RAW: &str = "https://raw.githubusercontent.com/";
const MAX_ROM: u64 = 32 << 20;

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Homebrew {
    #[serde(default)]
    pub format: u32,
    #[serde(default)]
    pub generated: String,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub slug: String,
    pub title: String,
    /// GBA, GBC, GB or NES.
    pub system: String,
    /// game, demo, tool or music.
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub devs: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub license: String,
    /// Its release date, as far as known (YYYY-MM-DD, YYYY-MM or YYYY).
    #[serde(default)]
    pub date: String,
    /// When it joined Homebrew Hub (YYYY-MM-DD).
    #[serde(default)]
    pub added: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub site: String,
    pub file: RomFile,
    /// A screenshot (PNG).
    #[serde(default)]
    pub shot: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
pub struct RomFile {
    pub url: String,
    pub name: String,
    pub size: u64,
    /// The git blob id: SHA-1 of "blob <size>\0" and the bytes.
    pub sha1: String,
}

/// The list as read, before each entry is taken on its own.
#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    format: u32,
    #[serde(default)]
    generated: String,
    #[serde(default)]
    entries: Vec<serde_json::Value>,
}

impl Homebrew {
    /// The list from `text`; an entry Port Store cannot read, or one that would
    /// download from elsewhere or write off the card, is left out.
    pub fn parse(text: &str) -> Result<Homebrew, String> {
        let raw: Raw = serde_json::from_str(text).map_err(|e| format!("homebrew: {e}"))?;
        if raw.format != 1 {
            return Err(format!("homebrew list format {} (this Port Store reads 1): update Port Store", raw.format));
        }
        let entries = raw
            .entries
            .into_iter()
            .filter_map(|v| serde_json::from_value::<Entry>(v).ok())
            .filter(Entry::sound)
            .collect();
        Ok(Homebrew { format: raw.format, generated: raw.generated, entries })
    }

    pub fn entry(&self, slug: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.slug == slug)
    }
}

impl Entry {
    fn sound(&self) -> bool {
        let slug = !self.slug.is_empty()
            && !self.slug.starts_with('.')
            && self.slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
        let sha = self.file.sha1.len() == 40 && self.file.sha1.chars().all(|c| c.is_ascii_hexdigit());
        slug && sha
            && !self.folder().is_empty()
            && self.file.url.starts_with(RAW)
            && (self.shot.is_empty() || (self.shot.starts_with(RAW) && self.shot.to_ascii_lowercase().ends_with(".png")))
            && self.file.size > 0
            && self.file.size <= MAX_ROM
    }

    /// OnionOS's folder for its system, under Roms/.
    pub fn folder(&self) -> &'static str {
        match self.system.as_str() {
            "GBA" => "GBA",
            "GBC" => "GBC",
            "GB" => "GB",
            "NES" => "FC",
            _ => "",
        }
    }

    pub fn ext(&self) -> &'static str {
        match self.system.as_str() {
            "GBA" => ".gba",
            "GBC" => ".gbc",
            "NES" => ".nes",
            _ => ".gb",
        }
    }

    /// The system as OnionOS's Games list names it.
    pub fn system_name(&self) -> &'static str {
        match self.system.as_str() {
            "GBA" => "Game Boy Advance",
            "GBC" => "Game Boy Color",
            "NES" => "Nintendo (FC)",
            _ => "Game Boy",
        }
    }
}

/// What was put on the card for an entry.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
pub struct Record {
    pub slug: String,
    pub title: String,
    pub sha1: String,
    /// The ROM, from the card's root.
    pub rom: String,
    /// Its box art, when Port Store put it there.
    #[serde(default)]
    pub art: String,
    #[serde(default)]
    pub date: String,
}

/// A file's git blob id.
pub fn blob_sha1(path: &Path, cancel: &AtomicBool) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let size = f.metadata()?.len();
    let mut h = sha1::Sha1::new();
    h.update(format!("blob {size}\0").as_bytes());
    let mut buf = vec![0; 1 << 16];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(fetch::hex(&h.finalize()))
}

/// Box art for MainUI from a screenshot: as it is, or made to fit 250 x 250 when larger
/// (a screenshot may be a photograph of a few megapixels; MainUI shows art at its size).
fn box_art(shot: &Path, to: &Path) -> io::Result<()> {
    let image = crate::ui::pictures::decode(shot).ok_or_else(|| io::Error::other("not a picture Port Store reads"))?;
    if image.w <= 250 && image.h <= 250 {
        return fs::copy(shot, to).map(|_| ());
    }
    let small = crate::ui::pictures::shrink(&image, 250, 250);
    let px: Vec<u32> = small.px.iter().map(|p| crate::ui::canvas::mix(0, *p, p >> 24)).collect();
    crate::ui::canvas::save_png(to, small.w, small.h, &px)
}

/// Where a picture of the list (a screenshot) is kept once downloaded.
pub fn picture_path(home: &Path, url: &str) -> PathBuf {
    let key = fetch::hex(&<sha2::Sha256 as sha2::Digest>::digest(url.as_bytes()));
    home.join("cache").join("hb-img").join(format!("{}.png", &key[..16]))
}

impl Store {
    fn records_dir(&self) -> PathBuf {
        self.card.home.join("installed").join("homebrew")
    }

    /// The list at hand, the newest of: the one Port Store came with, the one
    /// downloaded last, one brought into downloads/.
    pub fn load_homebrew(&self) -> Homebrew {
        let home = &self.card.home;
        [home.join(FILE), home.join("cache").join(FILE), home.join("downloads").join(FILE)]
            .into_iter()
            .filter_map(|p| Homebrew::parse(&fs::read_to_string(p).ok()?).ok())
            .max_by(|a, b| a.generated.cmp(&b.generated))
            .unwrap_or_default()
    }

    /// The list's address: next to the catalog's.
    pub fn homebrew_url(&self) -> Option<String> {
        self.index_url.rsplit_once('/').map(|(base, _)| format!("{base}/{FILE}"))
    }

    /// Download the list again.
    pub fn refresh_homebrew(&self, cancel: &AtomicBool) -> Result<Homebrew, String> {
        let url = self.homebrew_url().ok_or("no catalog address in settings.json (\"index\")")?;
        let to = self.card.home.join("cache").join(FILE);
        fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let new = to.with_extension(format!("json.new{}", SEQ.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_file(fetch::part_of(&new));
        self.fetcher.get(&url, &new, cancel, &mut |_| {}).map_err(|e| e.to_string())?;
        let list = fs::read_to_string(&new).map_err(|e| e.to_string()).and_then(|t| Homebrew::parse(&t));
        match &list {
            Ok(_) => fs::rename(&new, &to).map_err(|e| e.to_string())?,
            Err(_) => {
                let _ = fs::remove_file(&new);
            }
        }
        list
    }

    pub fn homebrew_record(&self, slug: &str) -> Option<Record> {
        let text = fs::read_to_string(self.records_dir().join(format!("{slug}.json"))).ok()?;
        let rec: Record = serde_json::from_str(&text).ok()?;
        // A ROM deleted by hand is no longer installed.
        (crate::index::card_path(&rec.rom) && !rec.rom.is_empty() && self.card.root.join(&rec.rom).exists()).then_some(rec)
    }

    /// Every entry installed, by slug (read once for the whole list).
    pub fn homebrew_records(&self) -> std::collections::HashMap<String, Record> {
        let Ok(dir) = fs::read_dir(self.records_dir()) else { return Default::default() };
        dir.filter_map(|e| e.ok())
            .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()))
            .filter_map(|slug| self.homebrew_record(&slug).map(|r| (slug, r)))
            .collect()
    }

    /// Installed (its version shown as the ROM's short id), an update when the list
    /// has another ROM for it, or absent.
    pub fn homebrew_state(&self, e: &Entry) -> State {
        state_of(self.homebrew_record(&e.slug).as_ref(), e)
    }
}

/// An entry's state from its record.
pub fn state_of(rec: Option<&Record>, e: &Entry) -> State {
    let short = |r: &Record| r.sha1.get(..7).unwrap_or(&r.sha1).to_string();
    match rec {
        Some(rec) if rec.sha1 == e.file.sha1 => State::Installed(short(rec)),
        Some(rec) => State::Update(short(rec)),
        None => State::Absent,
    }
}

impl Store {
    /// The ROM, checked: one put in downloads/ by hand, or downloaded.
    fn rom_file(&self, e: &Entry, cancel: &AtomicBool, progress: &mut dyn FnMut(Step)) -> io::Result<(PathBuf, bool)> {
        let name = Path::new(&e.file.name).file_name().ok_or_else(|| io::Error::other("bad file name in the list"))?;
        let by_hand = self.card.home.join("downloads").join(name);
        if by_hand.exists() {
            progress(Step::Check);
            if blob_sha1(&by_hand, cancel)? == e.file.sha1 {
                return Ok((by_hand, true));
            }
        }
        let path = self.card.home.join("cache").join(format!("{}-{}", &e.file.sha1[..12], name.to_string_lossy()));
        fs::create_dir_all(path.parent().unwrap())?;
        let _ = fs::remove_file(fetch::part_of(&path));
        if !path.exists() {
            let total = e.file.size;
            progress(Step::Download { done: 0, total });
            self.fetcher.get(&e.file.url, &path, cancel, &mut |done| progress(Step::Download { done, total }))?;
        }
        progress(Step::Check);
        if blob_sha1(&path, cancel)? != e.file.sha1 {
            let _ = fs::remove_file(&path);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the downloaded ROM is not the one in the list (checksum differs): nothing was changed",
            ));
        }
        Ok((path, false))
    }

    /// Download the entry's ROM, check it and put it in its system's folder, with its
    /// screenshot as box art (unless the folder has art of that name already).
    pub fn install_homebrew(&self, e: &Entry, cancel: &AtomicBool, progress: &mut dyn FnMut(Step)) -> io::Result<Record> {
        if !e.sound() {
            return Err(io::Error::other(format!("{}: not an entry Port Store installs", e.slug)));
        }
        let (file, by_hand) = self.rom_file(e, cancel, progress)?;
        progress(Step::Finish);
        let root = &self.card.root;
        let dir = format!("Roms/{}", e.folder());
        fs::create_dir_all(root.join(&dir))?;
        let old = self.homebrew_record(&e.slug);
        // An update keeps the ROM's name (its saves are by name); a new one never
        // replaces a file of the user's.
        let rom = match &old {
            Some(rec) => rec.rom.clone(),
            None => {
                let stem = miyooports::file_name(&e.title);
                let stem = if stem.is_empty() { e.slug.clone() } else { stem };
                needs::free_name(root, &dir, &format!("{stem}{}", e.ext()))
            }
        };
        if by_hand {
            fs::copy(&file, root.join(&rom))?;
        } else {
            move_file(&file, &root.join(&rom))?;
        }
        let stem = Path::new(&rom).file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let art = format!("{dir}/Imgs/{stem}.png");
        let mut ours = old.as_ref().is_some_and(|r| r.art == art);
        if !e.shot.is_empty() && (ours || !root.join(&art).exists()) {
            let shot = picture_path(&self.card.home, &e.shot);
            if !shot.exists() {
                let _ = fs::create_dir_all(shot.parent().unwrap());
                if self.fetcher.get(&e.shot, &shot, cancel, &mut |_| {}).is_err() {
                    let _ = fs::remove_file(&shot);
                }
            }
            if shot.exists() && fs::create_dir_all(root.join(&dir).join("Imgs")).is_ok() {
                ours = box_art(&shot, &root.join(&art)).is_ok();
            }
        }
        let rec = Record {
            slug: e.slug.clone(),
            title: e.title.clone(),
            sha1: e.file.sha1.clone(),
            rom,
            art: if ours { art } else { String::new() },
            date: crate::today(),
        };
        fs::create_dir_all(self.records_dir())?;
        let path = self.records_dir().join(format!("{}.json", e.slug));
        let tmp = path.with_extension("json.new");
        fs::write(&tmp, serde_json::to_string_pretty(&rec).map_err(io::Error::other)?)?;
        fs::rename(&tmp, &path)?;
        miyooports::reset_list(root, &dir);
        Ok(rec)
    }

    /// Remove what was put on the card for `slug`: the ROM and the box art Port Store
    /// added. Saves stay (OnionOS keeps them apart).
    pub fn remove_homebrew(&self, slug: &str) -> io::Result<Record> {
        let path = self.records_dir().join(format!("{slug}.json"));
        let rec: Record = serde_json::from_str(&fs::read_to_string(&path)?).map_err(io::Error::other)?;
        for f in [&rec.rom, &rec.art] {
            let ok = !f.is_empty() && crate::index::card_path(f) && f.starts_with("Roms/");
            if ok {
                let _ = fs::remove_file(self.card.root.join(f));
            }
        }
        if let Some(dir) = Path::new(&rec.rom).parent() {
            miyooports::reset_list(&self.card.root, &dir.to_string_lossy());
        }
        fs::remove_file(path)?;
        Ok(rec)
    }
}
