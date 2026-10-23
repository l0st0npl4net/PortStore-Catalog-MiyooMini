//! MiyooPorts: the games Port Store installs, as a system of their own in Games
//! (Emu/MiyooPorts), one launcher each (Roms/MiyooPorts/<Game>.sh) whatever the
//! release's layout: an app's folder, or a shortcut for OnionOS's Ports Collection.
//! The releases' own shortcuts, pictures and manuals for Ports go to a hidden folder
//! (Roms/MiyooPorts/.ports), so a game shows once, and Apps keeps only apps.
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::index::Port;

pub const DIR: &str = "Roms/MiyooPorts";
/// MainUI lists no name starting with a dot.
pub const HIDDEN: &str = "Roms/MiyooPorts/.ports/";
pub const SYSTEM: &str = "Emu/MiyooPorts";
pub const ICON: &str = "Icons/Default/miyooports.png";

static CONFIG: &str = include_str!("../dist/miyooports/config.json");
static LAUNCH: &str = include_str!("../dist/miyooports/launch.sh");
static ICON_PNG: &[u8] = include_bytes!("../../assets/miyooports.png");

const PORTS_FOLDERS: [&str; 3] = ["Shortcuts/", "Imgs/", "Manuals/"];

/// Where a file of the release goes: Ports Collection's folders are kept hidden.
pub fn relocate(path: String) -> String {
    for folder in PORTS_FOLDERS {
        if let Some(rest) = path.strip_prefix(&format!("Roms/PORTS/{folder}")) {
            return format!("{HIDDEN}{folder}{rest}");
        }
    }
    path
}

/// An app's own entry in Apps (App/<Name>/config.json) of a game: not installed,
/// the game is in MiyooPorts.
pub fn app_entry(port: &Port, path: &str) -> bool {
    !port.is_app()
        && path
            .strip_prefix("App/")
            .and_then(|rest| rest.strip_suffix("/config.json"))
            .is_some_and(|dir| !dir.is_empty() && !dir.contains('/'))
}

/// How a game in an app's folder starts, from its config.json.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AppLaunch {
    /// The app's folder, from the card's root (App/MarioKart64).
    pub dir: String,
    /// Its launcher in that folder (launch.sh, script/launch.sh).
    pub launch: String,
    #[serde(default)]
    pub label: String,
}

pub fn app_launch(config_path: &Path, dir: &str) -> AppLaunch {
    let json: serde_json::Value = fs::read_to_string(config_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let text = |key: &str| json.get(key).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let launch = text("launch");
    AppLaunch {
        dir: dir.to_string(),
        launch: if launch.is_empty() || launch.contains("..") { "launch.sh".into() } else { launch },
        label: text("label"),
    }
}

/// A file name MainUI lists the game by: no folder separators or quotes.
pub fn file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | '"' | ':' | '*' | '?' | '<' | '>' | '|' | '$' | '`') { ' ' } else { c })
        .collect();
    let clean: String = clean.chars().filter(|c| !c.is_control()).collect();
    // A leading dot would hide the game from MainUI's lists.
    clean.split_whitespace().collect::<Vec<_>>().join(" ").trim_start_matches('.').trim_start().to_string()
}

enum Start {
    /// cd into the app's folder and run its launcher there.
    App { dir: String, launch: String },
    /// A Ports Collection shortcut (kept hidden), run as Ports would run it.
    Shortcut(String),
}

/// The launchers (and their pictures) MiyooPorts gets for `port`, installed as
/// `files`: (path, bytes). Apps get none, unless released as a shortcut for Ports
/// (hidden like a game's): MiyooPorts is then the only place they start from.
/// `taken`: the files of other installed ports (in any case): a launcher of the same
/// name gets the port's id after its name, so neither overwrites the other.
pub fn launchers(
    root: &Path,
    home: &Path,
    port: &Port,
    files: &[String],
    apps: &[AppLaunch],
    taken: &std::collections::BTreeSet<String>,
) -> Vec<(String, Vec<u8>)> {
    if port.is_app() && !files.iter().any(|f| f.starts_with(&format!("{HIDDEN}Shortcuts/"))) {
        return Vec::new();
    }
    let mut starts: Vec<(String, Start, Option<String>)> = Vec::new();
    if let Some(sc) = &port.shortcut {
        if let Some((dir, launch)) = sc.launch.rsplit_once('/') {
            let image = Some(relocate(sc.image.clone())).filter(|i| !i.is_empty());
            starts.push((port.name.clone(), Start::App { dir: dir.into(), launch: launch.into() }, image));
        }
    } else {
        let shortcuts: Vec<&String> = files
            .iter()
            .filter(|f| f.starts_with(&format!("{HIDDEN}Shortcuts/")) && (f.ends_with(".port") || f.ends_with(".notfound")))
            .collect();
        for f in &shortcuts {
            let stem = Path::new(f.as_str()).file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let name = if shortcuts.len() == 1 { port.name.clone() } else { stem.clone() };
            let image = files.iter().find(|i| {
                i.starts_with(&format!("{HIDDEN}Imgs/"))
                    && Path::new(i.as_str()).file_stem().is_some_and(|s| s.to_string_lossy() == stem)
            });
            starts.push((name, Start::Shortcut(f.to_string()), image.cloned()));
        }
        if shortcuts.is_empty() {
            for app in apps {
                let name = match (apps.len(), app.label.is_empty()) {
                    (1, _) | (_, true) => port.name.clone(),
                    _ => app.label.clone(),
                };
                let start = Start::App { dir: app.dir.clone(), launch: app.launch.clone() };
                starts.push((name, start, None));
            }
        }
    }

    // In the port's folder of MiyooPorts, if it has one (its pictures stay in Imgs/:
    // MainUI looks for them there by name, whatever the folder).
    let folder = file_name(&port.folder);
    let at = |name: &str| match folder.is_empty() {
        true => format!("{DIR}/{name}.sh"),
        false => format!("{DIR}/{folder}/{name}.sh"),
    };
    let mut out = Vec::new();
    for (name, start, image) in starts {
        let mut name = file_name(&name);
        if taken.contains(&crate::install::fold(&at(&name))) {
            name = file_name(&format!("{name} ({})", port.id));
        }
        if name.is_empty() || out.iter().any(|(p, _): &(String, Vec<u8>)| p == &at(&name)) {
            continue;
        }
        out.push((at(&name), script(port, &name, &start).into_bytes()));
        // The picture: the release's own for Ports, else the catalog's.
        // The picture: the release's own for Ports, else the catalog's, else the one
        // already there (a new catalog's picture not downloaded yet).
        let picture = image
            .and_then(|i| fs::read(root.join(i)).ok())
            .or_else(|| {
                [home.join(&port.cover), home.join("cache").join(&port.cover)]
                    .into_iter()
                    .filter(|_| !port.cover.is_empty())
                    .find_map(|p| fs::read(p).ok())
            })
            .or_else(|| fs::read(root.join(format!("{DIR}/Imgs/{name}.png"))).ok());
        if let Some(png) = picture {
            out.push((format!("{DIR}/Imgs/{name}.png"), png));
        }
    }
    out
}

fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The launcher: first what the game needs from the user (said on screen when
/// missing), then the game.
fn script(port: &Port, name: &str, start: &Start) -> String {
    // A comment line: nothing in it may end it.
    let plain = |t: &str| t.chars().filter(|c| !c.is_control()).collect::<String>();
    let mut s = format!(
        "#!/bin/sh\n# {name}: made by Port Store for MiyooPorts ({id} {version}).\ncard=/mnt/SDCARD\n",
        id = plain(&port.id),
        version = plain(&port.version)
    );
    for need in port.needs.iter().filter(|n| !n.check.is_empty()) {
        // As OnionOS's ~Import ports: the file, or one of its name (any case) up to
        // two folders below its folder.
        let tests: Vec<String> = need
            .check
            .iter()
            .map(|c| {
                let (dir, file) = c.rsplit_once('/').unwrap_or((".", c));
                format!(
                    "[ -e \"$card\"/{p} ] || find \"$card\"/{d} -maxdepth 2 -iname {f} 2>/dev/null | grep -q .",
                    p = quoted(c),
                    d = quoted(dir),
                    f = quoted(file)
                )
            })
            .collect();
        // infoPanel breaks lines at a written "\n".
        let message = match need.hint.is_empty() {
            true => need.what.clone(),
            false => format!("{}\\n\\n{}", need.what, need.hint),
        };
        s += &format!(
            "if ! {{ {}; }}; then\n    \"$card\"/.tmp_update/bin/infoPanel --title {} --message {}\n    exit 0\nfi\n",
            tests.join(" || "),
            quoted(&format!("{name}: your files are missing")),
            quoted(&message)
        );
    }
    match start {
        Start::App { dir, launch } => {
            s += &format!(
                "cd \"$card\"/{} || exit 1\n\
                 # Sound through OnionOS's audioserver, as Apps have it.\n\
                 if pgrep audioserver > /dev/null 2>&1 && [ -f \"$card\"/miyoo/lib/libpadsp.so ]; then\n\
                 \x20   export LD_PRELOAD=\"$card\"/miyoo/lib/libpadsp.so\n\
                 fi\n\
                 exec sh ./{}\n",
                quoted(dir),
                quoted(launch)
            );
        }
        Start::Shortcut(file) => {
            s += &format!(
                "port=\"$card\"/{}\n\
                 # Through Ports Collection's launcher, which says when a port's files are missing.\n\
                 if [ -f \"$card\"/Emu/PORTS/launch.sh ]; then\n\
                 \x20   exec sh \"$card\"/Emu/PORTS/launch.sh \"$port\"\n\
                 fi\n\
                 cd \"$(dirname \"$port\")\" && exec sh \"$port\"\n",
                quoted(file)
            );
        }
    }
    s
}

/// Whether a shortcut of the port runs Ports Collection's launcher (which then has
/// to be on the card).
pub fn needs_ports_collection(root: &Path, files: &[String]) -> bool {
    files
        .iter()
        .filter(|f| f.starts_with(&format!("{HIDDEN}Shortcuts/")))
        .any(|f| fs::read_to_string(root.join(f)).is_ok_and(|t| t.contains("Emu/PORTS/")))
}

/// MiyooPorts in Games: its system folder and icon, put (back) on the card.
pub fn ensure_system(root: &Path) -> io::Result<()> {
    let system = root.join(SYSTEM);
    if !system.join("config.json").exists() {
        fs::create_dir_all(&system)?;
        fs::write(system.join("config.json"), CONFIG)?;
    }
    if !system.join("launch.sh").exists() {
        fs::write(system.join("launch.sh"), LAUNCH)?;
    }
    let icon = root.join(ICON);
    if !icon.exists() {
        fs::create_dir_all(icon.parent().unwrap())?;
        fs::write(icon, ICON_PNG)?;
    }
    fs::create_dir_all(root.join(DIR).join("Imgs"))
}

/// Port Store's own entry in MiyooPorts (an option): from Games it can go into
/// Favorites like a game.
const SELF_ENTRY: &str = "Roms/MiyooPorts/Port Store.sh";
const SELF_ART: &str = "Roms/MiyooPorts/Imgs/Port Store.png";
const SELF_MARK: &str = "# Port Store: its own entry in MiyooPorts.";
static SELF_ICON: &[u8] = include_bytes!("../../assets/icon.png");

/// Whether Port Store's own entry is in MiyooPorts.
pub fn self_entry(root: &Path) -> bool {
    fs::read_to_string(root.join(SELF_ENTRY)).is_ok_and(|t| t.contains(SELF_MARK))
}

/// Put Port Store's own entry in MiyooPorts, or take it away (only if it is Port
/// Store's: a game of that name stays).
pub fn set_self_entry(root: &Path, on: bool) -> io::Result<()> {
    if on {
        if root.join(SELF_ENTRY).exists() && !self_entry(root) {
            return Err(io::Error::other("a game in MiyooPorts is called Port Store already"));
        }
        ensure_system(root)?;
        let script = format!("#!/bin/sh\n{SELF_MARK}\ncd /mnt/SDCARD/App/PortStore || exit 1\nexec sh ./launch.sh\n");
        fs::write(root.join(SELF_ENTRY), script)?;
        fs::write(root.join(SELF_ART), SELF_ICON)?;
    } else if self_entry(root) {
        fs::remove_file(root.join(SELF_ENTRY))?;
        let _ = fs::remove_file(root.join(SELF_ART));
    }
    reset_list(root, DIR);
    Ok(())
}

/// MainUI lists a folder from its cache: dropped, it lists it afresh.
pub fn reset_list(root: &Path, dir: &str) {
    let dir = root.join(dir);
    let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
    for suffix in ["_cache2.db", "_cache6.db"] {
        let _ = fs::remove_file(dir.join(format!("{name}{suffix}")));
    }
}

// ---------------------------------------------------------------- OnionOS's own Ports
// Games > Ports lists Roms/PORTS/Shortcuts (Ports Collection's own system). Hidden,
// everything there (the ports put on the card by hand or by Package Manager, ~Import
// ports) moves to OWN, and each port gets a launcher in MiyooPorts' OWN_FOLDER: Ports,
// its folder empty, leaves Games. Shown again, all goes back. Port Store's own ports are
// in MiyooPorts either way; Ports Collection stays (MiyooPorts starts ports through it).

/// Where Ports' shortcuts are kept while Ports is hidden; there, Ports is hidden.
pub const OWN: &str = "Roms/MiyooPorts/.ports/own";
/// MiyooPorts' folder of their launchers.
pub const OWN_FOLDER: &str = "Ports";
const PORTS_LIST: &str = "Roms/PORTS/Shortcuts";
/// The pictures OWN's launchers got in Imgs/ (taken away with them).
const OWN_MADE: &str = "Roms/MiyooPorts/.ports/own.made";
/// The Ports entry that imports ports: pointless while Ports is hidden.
const IMPORT_ENTRY: &str = "~Import ports.miyoocmd";

pub fn ports_hidden(root: &Path) -> bool {
    root.join(OWN).is_dir()
}

/// Everything under `from` moved under `to` (folders merged, a file there replaced).
fn move_tree(from: &Path, to: &Path) -> io::Result<usize> {
    let mut n = 0;
    let Ok(entries) = fs::read_dir(from) else { return Ok(0) };
    for e in entries.flatten() {
        let (src, dest) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            fs::create_dir_all(&dest)?;
            n += move_tree(&src, &dest)?;
            let _ = fs::remove_dir(&src);
        } else {
            fs::create_dir_all(to)?;
            crate::install::move_file(&src, &dest)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Hide Games > Ports: its shortcuts into OWN, launchers for them in MiyooPorts.
pub fn hide_ports(root: &Path) -> io::Result<usize> {
    fs::create_dir_all(root.join(OWN))?;
    let n = move_tree(&root.join(PORTS_LIST), &root.join(OWN))?;
    own_launchers(root)?;
    reset_list(root, PORTS_LIST);
    Ok(n)
}

/// Show Games > Ports again: its shortcuts back, their launchers gone.
pub fn show_ports(root: &Path) -> io::Result<usize> {
    if !ports_hidden(root) {
        return Ok(0);
    }
    fs::create_dir_all(root.join(PORTS_LIST))?;
    let n = move_tree(&root.join(OWN), &root.join(PORTS_LIST))?;
    drop_own_launchers(root);
    let _ = fs::remove_dir_all(root.join(OWN));
    reset_list(root, PORTS_LIST);
    reset_list(root, DIR);
    Ok(n)
}

/// While Ports is hidden: what came into it since (Package Manager, ~Import ports, a
/// port copied by hand) joins the others. Whether anything did.
pub fn adopt_ports(root: &Path) -> io::Result<bool> {
    if !ports_hidden(root) {
        return Ok(false);
    }
    let n = move_tree(&root.join(PORTS_LIST), &root.join(OWN))?;
    if n > 0 {
        reset_list(root, PORTS_LIST);
    }
    // launchers again either way: a shortcut may have been removed by hand
    own_launchers(root)?;
    Ok(n > 0)
}

fn drop_own_launchers(root: &Path) {
    let _ = fs::remove_dir_all(root.join(DIR).join(OWN_FOLDER));
    if let Ok(made) = fs::read_to_string(root.join(OWN_MADE)) {
        for f in made.lines().filter(|f| crate::index::card_path(f) && f.starts_with(&format!("{DIR}/Imgs/"))) {
            let _ = fs::remove_file(root.join(f));
        }
    }
    let _ = fs::remove_file(root.join(OWN_MADE));
}

/// A launcher in MiyooPorts' Ports folder for every port of OWN (a .port, else its
/// .notfound: MiyooPorts shows a game whose files are missing, and says so when it
/// starts), with the picture Ports had for it.
fn own_launchers(root: &Path) -> io::Result<()> {
    drop_own_launchers(root);
    let mut found = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    walk(&root.join(OWN), &mut found);
    let mut made: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for path in &found {
        let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let ext = path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
        let wanted = match ext.as_str() {
            "port" | "miyoocmd" => file != IMPORT_ENTRY,
            // a .notfound only when there is no .port of its name
            "notfound" => !path.with_extension("port").exists(),
            _ => false,
        };
        if !wanted {
            continue;
        }
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let mut name = file_name(&stem);
        if name.is_empty() {
            continue;
        }
        while names.contains(&name) {
            name = format!("{name} (2)");
        }
        names.push(name.clone());
        let rel = path.strip_prefix(root).map_err(io::Error::other)?.to_string_lossy().into_owned();
        let port = Port { id: "ports".into(), name: name.clone(), version: "OnionOS".into(), ..Port::default() };
        let launcher = root.join(DIR).join(OWN_FOLDER).join(format!("{name}.sh"));
        fs::create_dir_all(launcher.parent().unwrap())?;
        fs::write(&launcher, script(&port, &name, &Start::Shortcut(rel)))?;
        // Ports' picture of it, by the shortcut's name; one already there is not touched
        let img = format!("{DIR}/Imgs/{name}.png");
        let ports_img = root.join(format!("Roms/PORTS/Imgs/{stem}.png"));
        if !root.join(&img).exists() && ports_img.is_file() {
            fs::create_dir_all(root.join(DIR).join("Imgs"))?;
            fs::copy(&ports_img, root.join(&img))?;
            made.push(img);
        }
    }
    if !names.is_empty() {
        ensure_system(root)?;
    }
    fs::write(root.join(OWN_MADE), made.join("\n"))?;
    reset_list(root, DIR);
    reset_list(root, &format!("{DIR}/{OWN_FOLDER}"));
    Ok(())
}
