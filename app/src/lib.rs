//! Port Store: installs ports on a Miyoo Mini running Onion, from a catalog of their
//! authors' own releases (index.json, built by index/build.py).
pub mod archive;
pub mod fetch;
pub mod index;
pub mod install;
pub mod needs;
pub mod ui;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use index::{Index, Port};
use install::{Card, Installed, Step};

pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 10 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1000 << 10 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{:.0} KB", (b as f64 / 1024.0).ceil()),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Absent,
    /// The port's folders are on the card, but Port Store did not put them there.
    ByHand,
    Installed(String),
    /// Installed, in an older version than the catalog's (the one installed).
    Update(String),
}

/// The card and how to reach the catalog. The catalog itself is the caller's: it is
/// replaced when downloaded again, while jobs share the store.
pub struct Store {
    pub card: Card,
    pub index_url: String,
    pub fetcher: fetch::Fetcher,
}

#[derive(serde::Deserialize, Default)]
struct Config {
    #[serde(default)]
    index: String,
}

impl Store {
    /// The store for the card at `root`, Port Store's own files in `home`.
    pub fn open(root: PathBuf, home: PathBuf) -> Store {
        let config: Config = fs::read_to_string(home.join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Store {
            fetcher: fetch::Fetcher::new(&home),
            card: Card { root, home },
            index_url: config.index,
        }
    }

    /// The newest catalog at hand: the one shipped with Port Store, or the last one
    /// downloaded.
    pub fn load_index(&self) -> Index {
        let home = &self.card.home;
        let read = |p: PathBuf| Index::parse(&fs::read_to_string(p).ok()?).ok();
        let bundled = read(home.join("index.json"));
        let downloaded = read(home.join("cache").join("index.json"));
        match (bundled, downloaded) {
            (Some(b), Some(d)) if b.generated > d.generated => b,
            (_, Some(d)) => d,
            (Some(b), None) => b,
            (None, None) => Index::default(),
        }
    }

    pub fn state(&self, port: &Port) -> State {
        match self.card.installed(&port.id) {
            Some(rec) if rec.version == port.version => State::Installed(rec.version),
            Some(rec) => State::Update(rec.version),
            None if self.card.found_by_hand(port) => State::ByHand,
            None => State::Absent,
        }
    }

    /// Onion's Ports Collection (Emu/PORTS), which Roms/PORTS shortcuts need.
    pub fn has_ports_collection(&self) -> bool {
        self.card.root.join("Emu/PORTS/config.json").exists()
    }

    /// Download the catalog again.
    pub fn refresh(&self, cancel: &AtomicBool) -> Result<Index, String> {
        if self.index_url.is_empty() {
            return Err("no catalog address in settings.json (\"index\")".into());
        }
        let to = self.card.home.join("cache").join("index.json");
        fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        let new = fetch::part_of(&to).with_extension("new");
        let _ = fs::remove_file(fetch::part_of(&new));
        self.fetcher
            .get(&self.index_url, &new, cancel, &mut |_| {})
            .map_err(|e| e.to_string())?;
        let text = fs::read_to_string(&new).map_err(|e| e.to_string())?;
        let index = Index::parse(&text)?;
        fs::rename(&new, &to).map_err(|e| e.to_string())?;
        Ok(index)
    }

    /// The port's release file, checked: one put in downloads/ by hand, or downloaded.
    fn release_file(
        &self,
        port: &Port,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Step),
    ) -> io::Result<(PathBuf, bool)> {
        let name = Path::new(&port.file.name)
            .file_name()
            .ok_or_else(|| io::Error::other("bad file name in the catalog"))?;
        let by_hand = self.card.home.join("downloads").join(name);
        if by_hand.exists() {
            progress(Step::Check);
            if fetch::sha256_file(&by_hand, cancel)? == port.file.sha256 {
                return Ok((by_hand, true));
            }
        }
        let path = self.card.home.join("cache").join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        if !path.exists() {
            let total = port.file.size;
            progress(Step::Download { done: 0, total });
            self.fetcher.get(&port.file.url, &path, cancel, &mut |done| {
                progress(Step::Download { done, total })
            })?;
        }
        progress(Step::Check);
        if fetch::sha256_file(&path, cancel)? != port.file.sha256 {
            let _ = fs::remove_file(&path);
            return Err(io::Error::other(
                "the downloaded file is not the one in the catalog (checksum differs); \
                 refresh the catalog and try again",
            ));
        }
        Ok((path, false))
    }

    /// Download (or find) the port's file, check it and install it.
    pub fn install(
        &self,
        port: &Port,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Step),
    ) -> io::Result<Installed> {
        let (file, by_hand) = self.release_file(port, cancel, progress)?;
        let rec = self.card.install(port, &file, progress);
        if !by_hand {
            let _ = fs::remove_file(&file);
        }
        let rec = rec?;
        if rec.shortcuts {
            self.mark_import();
        }
        Ok(rec)
    }

    pub fn remove(&self, id: &str, port: Option<&Port>) -> io::Result<Installed> {
        let rec = self.card.remove(id, port)?;
        if rec.shortcuts {
            self.mark_import();
        }
        Ok(rec)
    }

    /// launch.sh runs Onion's ~Import ports when Port Store exits, so new shortcuts
    /// show up (and those of ports still waiting for their game files stay hidden).
    fn mark_import(&self) {
        let _ = fs::write(self.card.home.join("cache").join("import"), b"");
    }
}
