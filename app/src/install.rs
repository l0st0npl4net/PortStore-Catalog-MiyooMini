//! Putting a port onto the card, updating and removing it. What went where is kept
//! in installed/<id>.json, so a port is removed or updated file by file, never by
//! wiping folders: saves, settings and game data in the port's folders stay.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::archive;
use crate::index::Port;
use crate::miyooports::{self, AppLaunch};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Installed {
    pub id: String,
    pub version: String,
    /// The release's date (YYYY-MM-DD), when known: a catalog of an older one is no update.
    #[serde(default)]
    pub date: String,
    /// Files put on the card, from its root.
    pub files: Vec<String>,
    /// Launchers in MiyooPorts (and, before 0.4, shortcuts in Ports).
    #[serde(default)]
    pub shortcuts: bool,
    /// How the game starts when it is in an app's folder (whose config.json, its
    /// entry in Apps, is not installed).
    #[serde(default)]
    pub apps: Vec<AppLaunch>,
    /// The release file's SHA-256: the same tag uploaded again is a new release.
    #[serde(default)]
    pub sha256: String,
    /// The port's `keep` when installed: kept also once it is gone from the catalog.
    #[serde(default)]
    pub keep: Vec<String>,
}

impl Installed {
    pub fn kept(&self, path: &str) -> bool {
        self.keep.iter().any(|k| crate::index::keeps(k, path))
    }
}

/// Progress of a job, to the screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Download { done: u64, total: u64 },
    Check,
    Unpack { done: u64, total: u64 },
    Finish,
}

/// OnionOS's ~Import ports looks for a port's files from here, also for those outside
/// it ("../../../App/MarioKart64"): without the folder that path leads nowhere and
/// the port is hidden as "not found". Ports Collection does not make it.
const GAMES: &str = "Roms/PORTS/Games";

/// Where a member goes on the card, if it goes there at all.
pub fn target(port: &Port, name: &str) -> Option<String> {
    let rel = name.strip_prefix(port.strip.as_str())?;
    // As index/build.py's target: into one folder, by the map, or as it is (take).
    let to = if !port.into.is_empty() {
        format!("{}{rel}", port.into)
    } else if !port.map.is_empty() {
        let [from, to] = port.map.iter().find(|[from, _]| rel.starts_with(from.as_str()))?;
        format!("{to}{}", &rel[from.len()..])
    } else if port.take.iter().any(|t| rel.starts_with(t.as_str())) {
        rel.to_string()
    } else {
        return None;
    };
    (!port.skip.iter().any(|s| to.starts_with(s.as_str()))).then(|| miyooports::relocate(to))
}

/// `.port` and `.notfound` are the same shortcut: ~Import ports renames one to the other.
pub fn twin(path: &str) -> Option<String> {
    if !path.starts_with("Roms/PORTS/Shortcuts/") {
        return None;
    }
    if let Some(stem) = path.strip_suffix(".port") {
        return Some(format!("{stem}.notfound"));
    }
    path.strip_suffix(".notfound").map(|stem| format!("{stem}.port"))
}

pub struct Card {
    pub root: PathBuf,
    /// Port Store's own folder: installed/, downloads/, cache/.
    pub home: PathBuf,
}

impl Card {
    pub fn record_path(&self, id: &str) -> PathBuf {
        self.home.join("installed").join(format!("{id}.json"))
    }

    pub fn installed(&self, id: &str) -> Option<Installed> {
        let text = fs::read_to_string(self.record_path(id)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn all_installed(&self) -> Vec<Installed> {
        let Ok(dir) = fs::read_dir(self.home.join("installed")) else {
            return Vec::new();
        };
        let mut out: Vec<Installed> = dir
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    fn write_record(&self, rec: &Installed) -> io::Result<()> {
        let path = self.record_path(&rec.id);
        fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.part");
        fs::write(&tmp, serde_json::to_vec_pretty(rec).unwrap())?;
        fs::rename(tmp, path)
    }

    /// Make Roms/PORTS/Games for installed ports with shortcuts, if it is missing
    /// (Port Store before 0.2 did not): whether it was, so ~Import ports runs again.
    pub fn repair_ports(&self) -> bool {
        let games = self.root.join(GAMES);
        let wanted = !games.exists() && self.all_installed().iter().any(|r| r.shortcuts);
        let made = wanted && fs::create_dir_all(games).is_ok();
        // MiyooPorts' own folder and icon, should they have gone.
        if self.all_installed().iter().any(|r| r.files.iter().any(|f| f.starts_with(miyooports::DIR))) {
            let _ = miyooports::ensure_system(&self.root);
        }
        made
    }

    /// Bring an installed port in line with how Port Store installs it now, without
    /// installing it again: shortcuts of Ports moved into MiyooPorts' hidden folder
    /// (Port Store before 0.4 had them in Ports), a game's entry in Apps taken away,
    /// files the catalog no longer installs (`skip`) removed, the MiyooPorts
    /// launchers made. Whether anything changed.
    pub fn conform(&self, port: &Port) -> io::Result<bool> {
        let Some(mut rec) = self.installed(&port.id) else {
            return Ok(false);
        };
        let mut changed = false;
        let mut gone = Vec::new();
        let mut files = Vec::new();
        for f in std::mem::take(&mut rec.files) {
            if port.skip.iter().any(|s| f.starts_with(s.as_str())) {
                // A file the user keeps is only left out of the record, never deleted.
                if !port.kept(&f) && !rec.kept(&f) {
                    gone.push(f);
                }
            } else if miyooports::app_entry(port, &f) {
                let dir = f.trim_end_matches("/config.json").to_string();
                rec.apps.push(miyooports::app_launch(&self.root.join(&f), &dir));
                gone.push(f);
            } else if f.starts_with("Roms/PORTS/Shortcuts/") || f.starts_with("Roms/PORTS/Imgs/") || f.starts_with("Roms/PORTS/Manuals/") {
                let to = miyooports::relocate(f.clone());
                // ~Import ports may have renamed the shortcut: whichever name is there.
                let from = [Some(f.clone()), twin(&f)]
                    .into_iter()
                    .flatten()
                    .find(|p| self.root.join(p).exists());
                // Shortcuts Port Store made itself are not wanted: the launcher starts it.
                let made = miyooports::file_name(&port.name);
                let made_by_us = port.shortcut.is_some()
                    && (f == format!("Roms/PORTS/Shortcuts/{made}.port") || f == format!("Roms/PORTS/Imgs/{made}.png"));
                if !made_by_us {
                    if let Some(from) = from {
                        let dest = self.root.join(&to);
                        fs::create_dir_all(dest.parent().unwrap())?;
                        move_file(&self.root.join(&from), &dest)?;
                    }
                    files.push(to);
                } else {
                    gone.push(f);
                }
                changed = true;
            } else {
                files.push(f);
            }
        }
        changed |= !gone.is_empty();
        self.remove_files(&port.id, &gone);
        rec.files = files;
        changed |= self.put_launchers(port, &mut rec.files, &rec.apps)?;
        rec.shortcuts = rec.files.iter().any(|f| f.starts_with(miyooports::DIR));
        if changed {
            self.write_record(&rec)?;
            miyooports::reset_list(&self.root, "Roms/PORTS/Shortcuts");
        }
        Ok(changed)
    }

    /// Write `port`'s MiyooPorts launchers (only those that differ), add them to
    /// `files`, take away launchers of it no longer made. Whether any changed.
    fn put_launchers(&self, port: &Port, files: &mut Vec<String>, apps: &[AppLaunch]) -> io::Result<bool> {
        let made = miyooports::launchers(&self.root, &self.home, port, files, apps, &self.others(&port.id));
        let mut changed = false;
        let old: Vec<String> = files
            .iter()
            .filter(|f| f.starts_with(&format!("{}/", miyooports::DIR)) && !f.starts_with(miyooports::HIDDEN))
            .cloned()
            .collect();
        let stale: Vec<String> = old.iter().filter(|f| !made.iter().any(|(p, _)| p == *f)).cloned().collect();
        if !stale.is_empty() {
            self.remove_files(&port.id, &stale);
            files.retain(|f| !stale.contains(f));
            changed = true;
        }
        if !made.is_empty() {
            miyooports::ensure_system(&self.root)?;
        }
        for (path, bytes) in made {
            let dest = self.root.join(&path);
            if fs::read(&dest).ok().as_deref() != Some(&bytes[..]) {
                fs::create_dir_all(dest.parent().unwrap())?;
                fs::write(&dest, &bytes)?;
                changed = true;
            }
            if !files.contains(&path) {
                files.push(path);
            }
        }
        if changed {
            miyooports::reset_list(&self.root, miyooports::DIR);
        }
        Ok(changed)
    }

    /// Whether `files` of a port run Ports Collection's launcher.
    pub fn needs_ports_collection(&self, files: &[String]) -> bool {
        miyooports::needs_ports_collection(&self.root, files)
    }

    /// A port of the catalog already on the card, put there by hand.
    pub fn found_by_hand(&self, port: &Port) -> bool {
        let marks = &port.install.marks;
        !marks.is_empty() && marks.iter().all(|m| self.root.join(m).is_file())
    }

    /// Install (or update to) `port` from `archive_path`, the checked release file.
    pub fn install(
        &self,
        port: &Port,
        archive_path: &Path,
        progress: &mut dyn FnMut(Step),
    ) -> io::Result<Installed> {
        // A 7z archive is unpacked whole by 7-Zip first, then read as a folder.
        if port.file.kind == "7z" {
            let raw = self.home.join("cache").join(format!("7z-{}", port.id));
            let free = free_bytes(&self.root);
            if free.is_some_and(|free| free < 2 * port.install.bytes + (8 << 20)) {
                return Err(io::Error::other(format!(
                    "not enough space: about {} needed to unpack, {} free",
                    crate::size(2 * port.install.bytes),
                    crate::size(free.unwrap())
                )));
            }
            progress(Step::Unpack { done: 0, total: 0 });
            archive::unpack_7z(archive_path, &raw)?;
            let mut as_dir = port.clone();
            as_dir.file.kind = "dir".into();
            let done = self.install(&as_dir, &raw, progress);
            let _ = remove_tree(&raw);
            return done;
        }
        let members = archive::members(archive_path, &port.file.kind)?;
        // Where each member goes. FAT32 tells no case apart: members that land on the
        // same path in any case (or twice) are one file there, the last one's
        // content under the first one's spelling.
        let mut spelling: BTreeMap<String, String> = BTreeMap::new();
        let mut wanted = Vec::new();
        for m in members.iter().filter(|m| !m.dir) {
            let Some(to) = target(port, &m.name) else { continue };
            // Only what goes onto the card has to be safe: a PC tool's
            // "licenses\x.txt" elsewhere in the archive is left alone.
            if !archive::safe_name(&m.name) || m.link || !crate::index::card_path(&to) {
                return Err(io::Error::other(format!("unsafe path or link in archive: {}", m.name)));
            }
            spelling.entry(fold(&to)).or_insert(to);
            wanted.push(m);
        }
        if wanted.is_empty() {
            return Err(io::Error::other("nothing in the file goes onto the card"));
        }
        let total: u64 = wanted.iter().map(|m| m.size).sum();
        let free = free_bytes(&self.root);
        if free.is_some_and(|free| free < total + total / 20 + (4 << 20)) {
            return Err(io::Error::other(format!(
                "not enough space: {} needed, {} free",
                crate::size(total),
                crate::size(free.unwrap())
            )));
        }

        // Unpack next to the card's own folders, then move each file into place: a
        // broken file stops the job before anything on the card is touched.
        let stage = self.home.join("cache").join(format!("stage-{}", port.id));
        remove_tree(&stage)?;
        fs::create_dir_all(&stage)?;
        let mut done = 0u64;
        let mut files: Vec<String> = Vec::new();
        progress(Step::Unpack { done, total });
        let in_place = |m: &archive::Member| target(port, &m.name).is_some();
        let unpacked = archive::extract(archive_path, &port.file.kind, in_place, |m, r| {
            let Some(to) = target(port, &m.name).and_then(|t| spelling.get(&fold(&t)).cloned()) else {
                return Ok(());
            };
            let path = stage.join(&to);
            fs::create_dir_all(path.parent().unwrap())?;
            let mut out = fs::File::create(&path)?;
            let mut buf = vec![0; 1 << 16];
            loop {
                let n = r.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                done += n as u64;
                progress(Step::Unpack { done, total });
            }
            if !files.contains(&to) {
                files.push(to);
            }
            Ok(())
        });
        if let Err(e) = unpacked {
            let _ = remove_tree(&stage);
            return Err(e);
        }
        progress(Step::Finish);

        // A game's entries in Apps: read for how it starts, not installed.
        let mut apps = Vec::new();
        files.retain(|f| {
            if !miyooports::app_entry(port, f) {
                return true;
            }
            apps.push(miyooports::app_launch(&stage.join(f), f.trim_end_matches("/config.json")));
            false
        });

        let before = self.installed(&port.id);
        let mut moved = Vec::new();
        let placed = (|| -> io::Result<()> {
            for f in &files {
                let dest = self.root.join(f);
                if port.kept(f) && dest.exists() {
                    moved.push(f.clone());
                    continue;
                }
                fs::create_dir_all(dest.parent().unwrap())?;
                if let Some(other) = twin(f) {
                    let _ = fs::remove_file(self.root.join(other));
                }
                move_file(&stage.join(f), &dest)?;
                moved.push(f.clone());
            }
            // The archive's folders, empty ones too: a port may count on them.
            for m in members.iter().filter(|m| m.dir && !m.link && archive::safe_name(&m.name)) {
                if let Some(to) = target(port, &m.name).filter(|t| crate::index::card_path(t)) {
                    fs::create_dir_all(self.root.join(to))?;
                }
            }
            Ok(())
        })();
        let _ = remove_tree(&stage);
        if let Err(e) = placed {
            // Half moved: what is on the card now is all recorded (the old version's
            // files and the new ones moved), so Remove still finds it, and the port
            // is offered again to finish.
            let mut known = before.as_ref().map(|b| b.files.clone()).unwrap_or_default();
            let have: BTreeSet<String> = known.iter().map(|k| fold(k)).collect();
            known.extend(moved.into_iter().filter(|m| !have.contains(&fold(m))));
            let rec = Installed {
                id: port.id.clone(),
                version: before.as_ref().map_or(INCOMPLETE.to_string(), |b| b.version.clone()),
                date: before.as_ref().map(|b| b.date.clone()).unwrap_or_default(),
                files: known,
                keep: port.keep.clone(),
                ..before.clone().unwrap_or_default()
            };
            let _ = self.write_record(&rec);
            return Err(e);
        }
        self.put_launchers(port, &mut files, &apps)?;
        // Ports Collection's launcher finds a port's files from Roms/PORTS/Games.
        if files.iter().any(|f| f.starts_with(miyooports::HIDDEN) || f.starts_with("Roms/PORTS/")) {
            fs::create_dir_all(self.root.join(GAMES))?;
        }

        // An update: what the old version had and the new one has not goes (by path
        // in any case: on FAT32 "Data/x" and "data/x" are one file).
        let now: BTreeSet<String> = files.iter().map(|f| fold(f)).collect();
        if let Some(old) = &before {
            let gone: Vec<String> = old
                .files
                .iter()
                .filter(|f| !now.contains(&fold(f)) && !port.kept(f) && !old.kept(f))
                .cloned()
                .collect();
            self.remove_files(&port.id, &gone);
        }
        let rec = Installed {
            id: port.id.clone(),
            version: port.version.clone(),
            date: port.date.clone(),
            sha256: port.file.sha256.clone(),
            shortcuts: files.iter().any(|f| f.starts_with(miyooports::DIR)),
            files,
            apps,
            keep: port.keep.clone(),
        };
        self.write_record(&rec)?;
        sync();
        Ok(rec)
    }

    /// Remove an installed port, leaving the user's files (`keep`) and anything in
    /// its folders Port Store did not put there.
    pub fn remove(&self, id: &str, port: Option<&Port>) -> io::Result<Installed> {
        let rec = self
            .installed(id)
            .ok_or_else(|| io::Error::other(format!("{id} is not installed by Port Store")))?;
        let files: Vec<String> = rec
            .files
            .iter()
            .filter(|f| !port.is_some_and(|p| p.kept(f)) && !rec.kept(f))
            .cloned()
            .collect();
        self.remove_files(id, &files);
        fs::remove_file(self.record_path(id))?;
        if rec.shortcuts {
            miyooports::reset_list(&self.root, miyooports::DIR);
        }
        sync();
        Ok(rec)
    }

    /// Files of other installed ports (in any case): never removed for this one.
    pub fn others(&self, id: &str) -> BTreeSet<String> {
        self.all_installed()
            .into_iter()
            .filter(|r| r.id != id)
            .flat_map(|r| r.files)
            .map(|f| fold(&f))
            .collect()
    }

    fn remove_files(&self, id: &str, files: &[String]) {
        let others = self.others(id);
        let mut dirs = BTreeSet::new();
        // A record's paths are the card's own: one that would lead off it is left.
        for f in files.iter().filter(|f| !others.contains(&fold(f)) && crate::index::card_path(f)) {
            let _ = fs::remove_file(self.root.join(f));
            if let Some(other) = twin(f) {
                let _ = fs::remove_file(self.root.join(other));
            }
            let mut p = Path::new(f).parent();
            while let Some(dir) = p.filter(|d| !d.as_os_str().is_empty()) {
                dirs.insert(dir.to_path_buf());
                p = dir.parent();
            }
        }
        // Deepest first; only folders left empty, and never the card's own ones.
        let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for d in dirs {
            // App, Roms/PORTS/Games, a Shortcuts category, MiyooPorts' own folders:
            // the card's or other ports'.
            let n = d.components().count();
            let shared = n == 1
                || d.starts_with("Roms") && n <= 3
                || d.starts_with("Roms/PORTS/Shortcuts") && n <= 4
                || d.starts_with(miyooports::HIDDEN.trim_end_matches('/')) && n <= 5;
            if !shared {
                let _ = fs::remove_dir(self.root.join(d));
            }
        }
    }
}

fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        // Nothing to move: the file in place stays.
        Err(e) if !from.exists() => Err(e),
        Err(_) => {
            // Onto a file FAT will not replace, or over another file system.
            let _ = fs::remove_file(to);
            fs::rename(from, to).or_else(|_| {
                fs::copy(from, to)?;
                fs::remove_file(from)
            })
        }
    }
}

/// A path as FAT32 compares it: in any case.
pub fn fold(path: &str) -> String {
    path.to_lowercase()
}

/// The version of a port whose install stopped half-way: offered again to finish.
pub const INCOMPLETE: &str = "(incomplete)";

pub fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

fn sync() {
    unsafe { libc::sync() };
}
