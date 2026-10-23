//! Port Store: installs ports on a Miyoo Mini running OnionOS, from a catalog of their
//! authors' own releases (index.json, built by index/build.py).
pub mod archive;
pub mod fetch;
pub mod homebrew;
pub mod index;
pub mod install;
pub mod miyooports;
pub mod needs;
pub mod ui;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use index::{Index, Port};
use install::{Card, Installed, Step};

/// Port Store's line in Apps (app/dist/config.json), with the updates after it.
const DESCRIPTION: &str = "Install and update ports";

/// Port Store's own entry in the catalog (recipes/portstore.json).
pub const SELF_ID: &str = "portstore";

/// The list of files for a computer to download, in downloads/.
pub const TO_DOWNLOAD: &str = "to-download.txt";

/// Today (UTC), as YYYY-MM-DD; `days` before it with `days_ago`.
pub fn today() -> String {
    days_ago(0)
}

pub fn days_ago(days: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64 / 86400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = now - days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 10 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1000 << 10 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{:.0} KB", (b as f64 / 1024.0).ceil()),
    }
}

/// A version as compared: the first run of numbers and dots ("v1.2.0-rc2" → 1.2.0,
/// "release1.5.1" → 1.5.1) with trailing zeros dropped (1.2 = 1.2.0), and what follows
/// it when that marks a pre-release ("-rc2", "beta1"), which comes before the bare
/// numbers.
fn version_key(v: &str) -> Option<(Vec<u64>, Option<String>)> {
    let start = v.find(|c: char| c.is_ascii_digit())?;
    let run: String = v[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let mut numbers: Vec<u64> = run.split('.').filter(|n| !n.is_empty()).filter_map(|n| n.parse().ok()).collect();
    while numbers.len() > 1 && numbers.last() == Some(&0) {
        numbers.pop();
    }
    let rest = v[start + run.len()..].trim_start_matches(['-', '_', '.', '+']).to_ascii_lowercase();
    let pre = ["alpha", "beta", "rc", "pre", "dev", "a", "b"]
        .iter()
        .any(|p| rest.starts_with(p) && rest[p.len()..].chars().next().is_none_or(|c| !c.is_ascii_alphabetic()));
    Some((numbers, pre.then_some(rest)))
}

/// Whether release `version` (of `date`) is newer than the one installed: by their
/// numbers when both have numbers of the same kind (not a date-like 2024 against a
/// 2), else by the release dates, else just by being another.
pub fn newer(version: &str, date: &str, installed: &str, installed_date: &str) -> bool {
    if let (Some((a, pa)), Some((b, pb))) = (version_key(version), version_key(installed)) {
        let year = |n: &[u64]| n.first().is_some_and(|&x| x >= 1000);
        if year(&a) == year(&b) {
            if a != b {
                return a > b;
            }
            match (&pa, &pb) {
                (None, Some(_)) => return true,
                (Some(_), None) => return false,
                (Some(x), Some(y)) if x != y => return x > y,
                // The same version ("1.2" and "v1.2.0"): newer only if released later.
                _ => return !date.is_empty() && !installed_date.is_empty() && date > installed_date,
            }
        }
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
        // Port Store itself is the program running: its version is its own, however
        // it came onto the card (an unzipped release has no install record).
        if port.id == SELF_ID {
            let me = format!("v{}", env!("CARGO_PKG_VERSION"));
            // By numbers only: "0.5.0" and "v0.5.0" are one version.
            let same = version_key(&port.version).map(|k| k.0) == version_key(&me).map(|k| k.0)
                && version_key(&port.version).and_then(|k| k.1).is_none();
            return match !same && newer(&port.version, "", &me, "") {
                true => State::Update(me),
                false => State::Installed(me),
            };
        }
        match self.card.installed(&port.id) {
            // The same tag, its file uploaded again (another checksum, a newer date).
            Some(rec)
                if rec.version == port.version
                    && !rec.sha256.is_empty()
                    && rec.sha256 != port.file.sha256
                    && port.date > rec.date =>
            {
                State::Update(rec.version)
            }
            Some(rec) if rec.version == port.version => State::Installed(rec.version),
            // Another's catalog numbers its builds as it likes: another one is newer.
            Some(rec) if !port.source.is_empty() => State::Update(rec.version),
            // Newer than the catalog's (a Port Store ahead of its catalog): no update.
            Some(rec) if !newer(&port.version, &port.date, &rec.version, &rec.date) => {
                State::Installed(rec.version)
            }
            Some(rec) => State::Update(rec.version),
            None if self.card.found_by_hand(port) => State::ByHand,
            None => State::Absent,
        }
    }

    /// OnionOS's Ports Collection (Emu/PORTS), which Roms/PORTS shortcuts need.
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
        // A name of its own per download: the one on start and a Y can overlap.
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let new = to.with_extension(format!("json.new{n}"));
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
        let text = fs::read_to_string(&new).map_err(|e| e.to_string());
        let index = text.and_then(|t| Index::parse(&t));
        match &index {
            Ok(_) => fs::rename(&new, &to).map_err(|e| e.to_string())?,
            Err(_) => {
                let _ = fs::remove_file(&new);
            }
        }
        index
    }

    /// The port's release file, checked: one put in downloads/ by hand, or downloaded.
    /// Downloads are kept by checksum (cache/<sha>-<name>), so a leftover of another
    /// release of the same name is never taken or resumed.
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
            // Not the catalog's file (an older release of the same name): set aside,
            // so it is not taken again and the list for a computer asks for the right one.
            let mut aside = by_hand.clone().into_os_string();
            aside.push(".wrong");
            let _ = fs::rename(&by_hand, aside);
        }
        let short = port.file.sha256.get(..12).unwrap_or("x");
        let path = self.card.home.join("cache").join(format!("{short}-{}", name.to_string_lossy()));
        fs::create_dir_all(path.parent().unwrap())?;
        // A part as long as the file, or longer, is no part of it.
        let part = fetch::part_of(&path);
        if fs::metadata(&part).is_ok_and(|m| port.file.size > 0 && m.len() >= port.file.size) {
            let _ = fs::remove_file(&part);
        }
        for attempt in 0..2 {
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
            if fetch::sha256_file(&path, cancel)? == port.file.sha256 {
                return Ok((path, false));
            }
            let _ = fs::remove_file(&path);
            // Once more from nothing (a resumed part that did not fit), then give up.
            if attempt == 1 {
                break;
            }
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the downloaded file is not the one in the catalog (checksum differs): \
             its author may have replaced it; the catalog catches up within a day",
        ))
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
        // Kept for another try after an error of the card (no space left), not after
        // a broken archive.
        let keep_file = rec.as_ref().is_err_and(|e| e.kind() != io::ErrorKind::InvalidData);
        if !by_hand && !keep_file {
            let _ = fs::remove_file(&file);
        }
        let rec = rec?;
        // Its shortcut runs Ports Collection's launcher: put Ports Collection on the
        // card first thing, as Package Manager would.
        if !self.has_ports_collection() && self.card.needs_ports_collection(&rec.files) {
            // Without it the game says so when started; the install itself stands.
            if let Err(e) = self.add_ports_collection() {
                eprintln!("portstore: Ports Collection: {e}");
            }
        }
        Ok(rec)
    }

    /// The ports `port` requires that are not on the card yet (or have an update),
    /// those they require in turn first: what to install before it. Err: one that is
    /// not in the catalog (its id).
    pub fn requirements(&self, index: &Index, port: &Port) -> Result<Vec<Port>, String> {
        fn walk(store: &Store, index: &Index, port: &Port, seen: &mut Vec<String>, out: &mut Vec<Port>) -> Result<(), String> {
            for id in &port.requires {
                if seen.contains(id) {
                    continue;
                }
                seen.push(id.clone());
                let need = index.port(id).ok_or_else(|| id.clone())?;
                walk(store, index, need, seen, out)?;
                if matches!(store.state(need), State::Absent | State::Update(_)) {
                    out.push(need.clone());
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        walk(self, index, port, &mut vec![port.id.clone()], &mut out)?;
        Ok(out)
    }

    /// The installed ports that require `id` (by the catalog), by name.
    pub fn required_by(&self, index: &Index, id: &str) -> Vec<String> {
        self.card
            .all_installed()
            .iter()
            .filter(|rec| rec.id != id)
            .filter_map(|rec| index.port(&rec.id))
            .filter(|p| p.requires.iter().any(|r| r == id))
            .map(|p| p.name.clone())
            .collect()
    }

    pub fn remove(&self, id: &str, port: Option<&Port>) -> io::Result<Installed> {
        if id == SELF_ID {
            return Err(io::Error::other("Port Store does not remove itself: delete App/PortStore on a computer"));
        }
        if let Some(rec) = self.card.installed(id) {
            self.uninstall_hooks(&rec.files);
        }
        self.card.remove(id, port)
    }

    /// A port's own cleanup before its files go (for what it changed outside its
    /// folders): `portstore-uninstall.sh`, or `mixtape-uninstall.sh` as Mixtape names
    /// it, among its files, run with sh in its folder, with PORTSTORE_UNINSTALL=1,
    /// MIXTAPE_UNINSTALL=1 and SDCARD set, for 30 seconds at most. However it ends,
    /// the files go; what it says goes to the log.
    fn uninstall_hooks(&self, files: &[String]) {
        for f in files {
            let name = Path::new(f).file_name().unwrap_or_default();
            let path = self.card.root.join(f);
            if !matches!(name.to_str(), Some("portstore-uninstall.sh" | "mixtape-uninstall.sh")) || !path.is_file() {
                continue;
            }
            let child = std::process::Command::new("sh")
                .arg(&path)
                .current_dir(path.parent().unwrap_or(&self.card.root))
                .env("PORTSTORE_UNINSTALL", "1")
                .env("MIXTAPE_UNINSTALL", "1")
                .env("SDCARD", &self.card.root)
                .spawn();
            let Ok(mut child) = child else {
                eprintln!("portstore: {f}: could not run it");
                continue;
            };
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            eprintln!("portstore: {f}: {status}");
                        }
                        break;
                    }
                    Ok(None) if start.elapsed() > std::time::Duration::from_secs(30) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        eprintln!("portstore: {f}: stopped after 30 seconds");
                        break;
                    }
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                    Err(e) => {
                        eprintln!("portstore: {f}: {e}");
                        break;
                    }
                }
            }
        }
    }

    /// Ports of the catalog new on this card since the last look: their ids, and for
    /// every port the day it was first seen ("" for those there at the very first
    /// look, which announces nothing). Kept in cache/seen.json.
    pub fn arrivals(&self, index: &Index) -> (Vec<String>, std::collections::HashMap<String, String>) {
        let path = self.card.home.join("cache").join("seen.json");
        let first = !path.exists();
        let mut seen: std::collections::BTreeMap<String, String> = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let mut new = Vec::new();
        if !index.ports.is_empty() {
            let today = today();
            for p in index.ports.iter().filter(|p| !p.dev && p.id != SELF_ID) {
                if !seen.contains_key(&p.id) {
                    seen.insert(p.id.clone(), if first { String::new() } else { today.clone() });
                    if !first {
                        new.push(p.id.clone());
                    }
                }
            }
            if first || !new.is_empty() {
                let _ = fs::create_dir_all(path.parent().unwrap());
                let tmp = path.with_extension("json.new");
                if serde_json::to_string(&seen).ok().is_some_and(|t| fs::write(&tmp, t).is_ok()) {
                    let _ = fs::rename(&tmp, &path);
                }
            }
        }
        (new, seen.into_iter().collect())
    }

    /// Where Package Manager keeps Ports Collection, ready to install.
    fn ports_collection_package(&self) -> PathBuf {
        self.card.root.join("App/PackageManager/data/Emu/Ports Collection")
    }

    /// Install OnionOS's Ports Collection as Package Manager does: its files copied to
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

    /// Say the number of updates in Port Store's description in Apps (OnionOS's
    /// MainUI shows it under the name), so they are seen without opening it. Its
    /// config.json is changed only when the line changes; OnionOS's own keys stay.
    pub fn note_updates(&self, updates: usize) {
        let path = self.card.home.join("config.json");
        let Ok(text) = fs::read_to_string(&path) else { return };
        let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) else { return };
        let line = match updates {
            0 => DESCRIPTION.to_string(),
            1 => format!("{DESCRIPTION} · 1 update"),
            n => format!("{DESCRIPTION} · {n} updates"),
        };
        if json.get("description").and_then(|d| d.as_str()) == Some(line.as_str()) {
            return;
        }
        let Some(map) = json.as_object_mut() else { return };
        map.insert("description".into(), line.into());
        if let Ok(out) = serde_json::to_string_pretty(&json) {
            // Whole or not at all: a half-written config.json takes Port Store out of Apps.
            let tmp = path.with_extension("json.part");
            if fs::write(&tmp, out + "\n").is_ok() && fs::rename(&tmp, &path).is_ok() {
                unsafe { libc::sync() };
            }
        }
    }

    /// Put right what an earlier Port Store (or catalog) left otherwise on the card.
    pub fn repair(&self, index: &Index) {
        let mut changed = self.card.repair_ports();
        // Games > Ports hidden: what came into it since goes to MiyooPorts too
        if let Err(e) = miyooports::adopt_ports(&self.card.root) {
            eprintln!("portstore: Ports: {e}");
        }
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

    /// launch.sh runs OnionOS's ~Import ports when Port Store exits, so new shortcuts
    /// show up (and those of ports still waiting for their game files stay hidden).
    fn mark_import(&self) {
        let _ = fs::create_dir_all(self.card.home.join("cache"));
        let _ = fs::write(self.card.home.join("cache").join("import"), b"");
    }
}
