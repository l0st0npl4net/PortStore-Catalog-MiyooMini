//! portstore            the screens (on the device)
//! portstore list       the catalog and what is installed
//! portstore install ID / remove ID / needs ID / refresh
//! portstore homebrew [refresh | install SLUG | remove SLUG]   Homebrew Hub's list
//! portstore shots DIR [ID...]   screens as PNGs
//!
//! PORTSTORE_CARD is the SD card (/mnt/SDCARD), PORTSTORE_HOME Port Store's folder
//! (the one the program is in).
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use portstore::install::Step;
use portstore::needs::{self, Roms};
use portstore::{size, State, Store};

fn main() -> ExitCode {
    let card = std::env::var_os("PORTSTORE_CARD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/SDCARD"));
    let home = std::env::var_os("PORTSTORE_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(std::env::current_exe().ok()?.parent()?.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let store = Store::open(card, home);
    let cancel = AtomicBool::new(false);
    let result = match args.first().map(String::as_str) {
        None => portstore::ui::run(store),
        Some("list") => {
            let index = store.load_index();
            println!("catalog {} ({} ports, {} by hand)", index.generated, index.ports.len(), index.manual.len());
            for p in &index.ports {
                let state = match store.state(p) {
                    State::Absent => String::new(),
                    State::ByHand => "on card (by hand)".into(),
                    State::Installed(v) => format!("installed {v}"),
                    State::Update(v) => format!("update from {v}"),
                };
                println!("{:24} {:16} {:>8}  {}", p.id, p.version, size(p.file.size), state);
            }
            Ok(())
        }
        Some("install") => each(&args[1..], |id| {
            let index = store.load_index();
            let port = index.port(id).ok_or(format!("{id}: not in the catalog"))?;
            let mut last = String::new();
            let rec = store
                .install(port, &cancel, &mut |step| {
                    let now = match step {
                        Step::Download { .. } => "downloading",
                        Step::Check => "checking",
                        Step::Unpack { .. } => "unpacking",
                        Step::Finish => "placing",
                    };
                    if now != last {
                        eprintln!("{id}: {now}");
                        last = now.to_string();
                    }
                })
                .map_err(|e| format!("{id}: {e}"))?;
            println!("{id}: installed {} ({} files)", rec.version, rec.files.len());
            Ok(())
        }),
        Some("remove") => each(&args[1..], |id| {
            let index = store.load_index();
            let rec = store.remove(id, index.port(id)).map_err(|e| format!("{id}: {e}"))?;
            println!("{id}: removed {} ({} files)", rec.version, rec.files.len());
            Ok(())
        }),
        Some("needs") => each(&args[1..], |id| {
            let index = store.load_index();
            let port = index.port(id).ok_or(format!("{id}: not in the catalog"))?;
            let mut roms = Roms::open(&store.card.home);
            let have = needs::check(&store.card.root, port, &mut roms, &mut |p| {
                eprintln!("hashing {}", p.display())
            });
            for (need, have) in port.needs.iter().zip(have) {
                println!("{id}: {} -> {have:?}", need.what);
            }
            Ok(())
        }),
        Some("refresh") => store.refresh(&cancel).map(|index| {
            println!("catalog {}: {} ports", index.generated, index.ports.len());
        }),
        Some("homebrew") => match (args.get(1).map(String::as_str), args.get(2)) {
            (None, _) => {
                let list = store.load_homebrew();
                println!("homebrew list {} ({} entries)", list.generated, list.entries.len());
                for e in &list.entries {
                    println!("{:32} {:4} {:6} {:>8}  {:?}", e.slug, e.system, e.kind, size(e.file.size), store.homebrew_state(e));
                }
                Ok(())
            }
            (Some("refresh"), _) => store.refresh_homebrew(&cancel).map(|l| println!("homebrew list {}: {} entries", l.generated, l.entries.len())),
            (Some("install"), Some(slug)) => (|| {
                let list = store.load_homebrew();
                let e = list.entry(slug).ok_or(format!("{slug}: not in the homebrew list"))?;
                let rec = store.install_homebrew(e, &cancel, &mut |_| {}).map_err(|err| format!("{slug}: {err}"))?;
                println!("{slug}: installed as {}{}", rec.rom, if rec.art.is_empty() { String::new() } else { format!(", box art {}", rec.art) });
                Ok(())
            })(),
            (Some("remove"), Some(slug)) => store.remove_homebrew(slug).map(|r| println!("{slug}: removed {}", r.rom)).map_err(|e| format!("{slug}: {e}")),
            _ => Err("homebrew [refresh | install SLUG | remove SLUG]".into()),
        },
        Some("shots") if args.len() > 1 => portstore::ui::shots(store, &PathBuf::from(&args[1]), &args[2..]),
        Some(other) => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("portstore: {e}");
            ExitCode::FAILURE
        }
    }
}

fn each(ids: &[String], mut f: impl FnMut(&str) -> Result<(), String>) -> Result<(), String> {
    if ids.is_empty() {
        return Err("which port? (an id from `portstore list`)".into());
    }
    ids.iter().try_for_each(|id| f(id))
}
