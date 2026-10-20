//! Putting a port onto the card, updating and removing it. What went where is kept
//! in installed/<id>.json, so a port is removed or updated file by file, never by
//! wiping folders: saves, settings and game data in the port's folders stay.
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::archive;
use crate::index::Port;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Installed {
    pub id: String,
    pub version: String,
    /// Files put on the card, from its root.
    pub files: Vec<String>,
    /// Shortcuts for Onion's Ports Collection: ~Import ports has to look at them.
    #[serde(default)]
    pub shortcuts: bool,
}

/// Progress of a job, to the screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Download { done: u64, total: u64 },
    Check,
    Unpack { done: u64, total: u64 },
    Finish,
}

const SHORTCUTS: &str = "Roms/PORTS/Shortcuts/";
/// Onion's ~Import ports looks for a port's files from here, also for those outside
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
    (!port.skip.iter().any(|s| to.starts_with(s.as_str()))).then_some(to)
}

/// The Ports shortcut (and its picture) Port Store makes for `port`: (path, bytes).
pub fn made_shortcut(root: &Path, port: &Port) -> Vec<(String, Vec<u8>)> {
    let Some(sc) = &port.shortcut else {
        return Vec::new();
    };
    let Some((dir, launch)) = sc.launch.rsplit_once('/') else {
        return Vec::new();
    };
    // A file name Onion lists the game by: no folder separators or quotes.
    let name: String = port
        .name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | '"' | ':' | '*' | '?' | '<' | '>' | '|') { ' ' } else { c })
        .collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = format!(
        "#!/bin/sh\n\
         # {name}: made by Port Store for Onion's Ports; the game is in {dir}.\n\
         GameName=\"{name}\"\n\
         GameDir=\"../../../{dir}\"\n\
         GameDataFile=\"{launch}\"\n\
         cd \"/mnt/SDCARD/{dir}\" || exit 1\n\
         # Sound through Onion's audioserver, as Apps have it.\n\
         if pgrep audioserver > /dev/null 2>&1 && [ -f /mnt/SDCARD/miyoo/lib/libpadsp.so ]; then\n\
         \x20   export LD_PRELOAD=/mnt/SDCARD/miyoo/lib/libpadsp.so\n\
         fi\n\
         exec sh \"./{launch}\"\n"
    );
    let mut out = vec![(format!("{SHORTCUTS}{name}.port"), text.into_bytes())];
    if !sc.image.is_empty() {
        if let Ok(png) = fs::read(root.join(&sc.image)) {
            out.push((format!("Roms/PORTS/Imgs/{name}.png"), png));
        }
    }
    out
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
        wanted && fs::create_dir_all(games).is_ok()
    }

    /// Bring an installed port in line with how the catalog installs it now, without
    /// installing it again: files it no longer installs (`skip`) go, a shortcut it now
    /// makes is made. Whether anything changed.
    pub fn conform(&self, port: &Port) -> io::Result<bool> {
        let Some(mut rec) = self.installed(&port.id) else {
            return Ok(false);
        };
        let skipped: Vec<String> = rec
            .files
            .iter()
            .filter(|f| port.skip.iter().any(|s| f.starts_with(s.as_str())))
            .cloned()
            .collect();
        let made: Vec<(String, Vec<u8>)> = made_shortcut(&self.root, port)
            .into_iter()
            .filter(|(path, _)| !rec.files.contains(path))
            .collect();
        if skipped.is_empty() && made.is_empty() {
            return Ok(false);
        }
        self.remove_files(&port.id, &skipped);
        rec.files.retain(|f| !skipped.contains(f));
        for (path, bytes) in made {
            let dest = self.root.join(&path);
            fs::create_dir_all(dest.parent().unwrap())?;
            fs::write(dest, bytes)?;
            rec.files.push(path);
        }
        rec.shortcuts = rec.files.iter().any(|f| f.starts_with(SHORTCUTS));
        if rec.shortcuts {
            fs::create_dir_all(self.root.join(GAMES))?;
        }
        self.write_record(&rec)?;
        Ok(true)
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
        let members = archive::members(archive_path, &port.file.kind)?;
        let wanted: Vec<&archive::Member> = members
            .iter()
            .filter(|m| !m.dir && target(port, &m.name).is_some())
            .collect();
        // Only what goes onto the card has to be safe: a PC tool's "licenses\x.txt"
        // elsewhere in the archive is left alone.
        if let Some(m) = wanted.iter().find(|m| !archive::safe_name(&m.name) || m.link) {
            return Err(io::Error::other(format!("unsafe path or link in archive: {}", m.name)));
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
        let mut files = Vec::new();
        progress(Step::Unpack { done, total });
        let unpacked = archive::extract(archive_path, &port.file.kind, |m, r| {
            let Some(to) = target(port, &m.name) else {
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
            files.push(to);
            Ok(())
        });
        if let Err(e) = unpacked {
            let _ = remove_tree(&stage);
            return Err(e);
        }
        progress(Step::Finish);

        let before = self.installed(&port.id);
        for f in &files {
            let dest = self.root.join(f);
            if port.kept(f) && dest.exists() {
                continue;
            }
            fs::create_dir_all(dest.parent().unwrap())?;
            if let Some(other) = twin(f) {
                let _ = fs::remove_file(self.root.join(other));
            }
            move_file(&stage.join(f), &dest)?;
        }
        let _ = remove_tree(&stage);
        // The archive's folders, empty ones too: a port may count on them.
        for m in members.iter().filter(|m| m.dir && !m.link && archive::safe_name(&m.name)) {
            if let Some(to) = target(port, &m.name) {
                fs::create_dir_all(self.root.join(to))?;
            }
        }
        for (path, bytes) in made_shortcut(&self.root, port) {
            let dest = self.root.join(&path);
            fs::create_dir_all(dest.parent().unwrap())?;
            if let Some(other) = twin(&path) {
                let _ = fs::remove_file(self.root.join(other));
            }
            fs::write(&dest, bytes)?;
            files.push(path);
        }
        if files.iter().any(|f| f.starts_with(SHORTCUTS)) {
            fs::create_dir_all(self.root.join(GAMES))?;
        }

        // An update: what the old version had and the new one has not goes.
        let now: BTreeSet<&String> = files.iter().collect();
        if let Some(old) = &before {
            let gone: Vec<String> = old
                .files
                .iter()
                .filter(|f| !now.contains(f) && !port.kept(f))
                .cloned()
                .collect();
            self.remove_files(&port.id, &gone);
        }
        let rec = Installed {
            id: port.id.clone(),
            version: port.version.clone(),
            shortcuts: files.iter().any(|f| f.starts_with("Roms/PORTS/Shortcuts/")),
            files,
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
            .filter(|f| !port.is_some_and(|p| p.kept(f)))
            .cloned()
            .collect();
        self.remove_files(id, &files);
        fs::remove_file(self.record_path(id))?;
        sync();
        Ok(rec)
    }

    /// Files of other installed ports: never removed for this one.
    fn others(&self, id: &str) -> BTreeSet<String> {
        self.all_installed()
            .into_iter()
            .filter(|r| r.id != id)
            .flat_map(|r| r.files)
            .collect()
    }

    fn remove_files(&self, id: &str, files: &[String]) {
        let others = self.others(id);
        let mut dirs = BTreeSet::new();
        for f in files.iter().filter(|f| !others.contains(*f)) {
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
            // App, Roms/PORTS/Games, a Shortcuts category: the card's or other ports'.
            let n = d.components().count();
            let shared = n == 1
                || d.starts_with("Roms") && n <= 3
                || d.starts_with("Roms/PORTS/Shortcuts") && n <= 4;
            if !shared {
                let _ = fs::remove_dir(self.root.join(d));
            }
        }
    }
}

fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Over a different file system, or onto a file FAT will not replace.
            let _ = fs::remove_file(to);
            fs::rename(from, to).or_else(|_| {
                fs::copy(from, to)?;
                fs::remove_file(from)
            })
        }
    }
}

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
