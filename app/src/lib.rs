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

/// The list of files for a computer to download, in downloads/.
pub const TO_DOWNLOAD: &str = "to-download.txt";

pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 10 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1000 << 10 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{:.0} KB", (b as f64 / 1024.0).ceil()),
    }
}

/// Whether release `version` (of `date`) is newer than the one installed: by the
/// numbers in the versions when both have some ("v0.3.0", "release1.5.1"), else by
/// date, else just by being another.
pub fn newer(version: &str, date: &str, installed: &str, installed_date: &str) -> bool {
    let numbers = |v: &str| -> Vec<u64> {
        v.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse().ok()).collect()
    };
    let (a, b) = (numbers(version), numbers(installed));
    if !a.is_empty() && !b.is_empty() && a != b {
        return a > b;
    }
    if !date.is_empty() && !installed_date.is_empty() && date != installed_date {
        return date > installed_date;
    }
    version != installed
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
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

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
        // Shipped with Port Store, downloaded by it, or brought on a computer into
        // downloads/: the newest.
        [
            home.join("index.json"),
            home.join("cache").join("index.json"),
            home.join("downloads").join("index.json"),
        ]
        .into_iter()
        .filter_map(read)
        .max_by(|a, b| a.generated.cmp(&b.generated))
        .unwrap_or_default()
    }

    pub fn state(&self, port: &Port) -> State {
        match self.card.installed(&port.id) {
            Some(rec) if rec.version == port.version => State::Installed(rec.version),
            // Newer than the catalog's (a Port Store ahead of its catalog): no update.
            Some(rec) if !newer(&port.version, &port.date, &rec.version, &rec.date) => {
                State::Installed(rec.version)
            }
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
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotConnected => {
                    self.list_for_computer(&[]);
                    format!(
                        "{e}. Without Wi-Fi: the catalog's address is in App/PortStore/downloads/{TO_DOWNLOAD}; \
                         save it there as index.json from a computer."
                    )
                }
                _ => e.to_string(),
            })?;
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
            let got = self.fetcher.get(&port.file.url, &path, cancel, &mut |done| {
                progress(Step::Download { done, total })
            });
            if let Err(e) = got {
                if e.kind() == io::ErrorKind::NotConnected {
                    self.list_for_computer(&[port]);
                    return Err(io::Error::new(
                        e.kind(),
                        format!(
                            "No connection. The file is listed in App/PortStore/downloads/{TO_DOWNLOAD}: \
                             download it on a computer into that folder, then install again."
                        ),
                    ));
                }
                return Err(e);
            }
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

    /// Add the release files of `ports` to downloads/to-download.txt, the list for a
    /// computer: the newest catalog first, then each file's name and address.
    pub fn list_for_computer(&self, ports: &[&Port]) {
        let path = self.card.home.join("downloads").join(TO_DOWNLOAD);
        let mut files: Vec<(String, String)> = Vec::new();
        // Files listed before stay, unless already brought.
        if let Ok(text) = fs::read_to_string(&path) {
            let lines: Vec<&str> = text.lines().collect();
            for pair in lines.windows(2) {
                if let (Some(name), Some(url)) = (pair[0].strip_prefix("File: "), pair[1].strip_prefix("  ")) {
                    files.push((name.to_string(), url.to_string()));
                }
            }
        }
        for p in ports {
            if !files.iter().any(|(n, _)| *n == p.file.name) {
                files.push((p.file.name.clone(), p.file.url.clone()));
            }
        }
        let downloads = self.card.home.join("downloads");
        files.retain(|(name, _)| !downloads.join(name).exists());
        let mut text = String::from(
            "Port Store: files to download on a computer and copy into this folder\n\
             (App/PortStore/downloads on the card), each under the name given here.\n\n",
        );
        if !self.index_url.is_empty() {
            text += &format!("Newest catalog, saved here as index.json:\n  {}\n\n", self.index_url);
        }
        for (name, url) in &files {
            text += &format!("File: {name}\n  {url}\n");
        }
        let _ = fs::create_dir_all(&downloads);
        let _ = fs::write(path, text);
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

    /// Where Package Manager keeps Ports Collection, ready to install.
    fn ports_collection_package(&self) -> PathBuf {
        self.card.root.join("App/PackageManager/data/Emu/Ports Collection")
    }

    /// Install Onion's Ports Collection as Package Manager does: its files copied to
    /// the card, then its install.sh run where it lies.
    pub fn add_ports_collection(&self) -> io::Result<()> {
        let package = self.ports_collection_package();
        if !package.is_dir() {
            return Err(io::Error::other(
                "Package Manager's Ports Collection is not on the card: add it in Package Manager (Emulators)",
            ));
        }
        for entry in fs::read_dir(&package)? {
            let entry = entry?;
            copy_tree(&entry.path(), &self.card.root.join(entry.file_name()))?;
        }
        let main = package.join("Emu/PORTS");
        if main.join("install.sh").is_file() {
            let status = std::process::Command::new("sh")
                .arg("./install.sh")
                .current_dir(&main)
                .status()?;
            if !status.success() {
                return Err(io::Error::other(format!("Ports Collection's install.sh: {status}")));
            }
        }
        unsafe { libc::sync() };
        self.mark_import();
        Ok(())
    }

    /// Put right what an earlier Port Store (or catalog) left otherwise on the card.
    pub fn repair(&self, index: &Index) {
        let mut changed = self.card.repair_ports();
        for port in &index.ports {
            match self.card.conform(port) {
                Ok(true) => changed = true,
                Ok(false) => {}
                Err(e) => eprintln!("portstore: {}: {e}", port.id),
            }
        }
        if changed {
            self.mark_import();
        }
    }

    /// launch.sh runs Onion's ~Import ports when Port Store exits, so new shortcuts
    /// show up (and those of ports still waiting for their game files stay hidden).
    fn mark_import(&self) {
        let _ = fs::create_dir_all(self.card.home.join("cache"));
        let _ = fs::write(self.card.home.join("cache").join("import"), b"");
    }
}
