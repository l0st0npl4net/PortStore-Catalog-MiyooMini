//! Installing, updating and removing ports on a card in a temporary folder, from
//! archives made here.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use portstore::archive;
use portstore::fetch::hex;
use portstore::index::{File, Index, Install, Need, Port, Shortcut};
use portstore::install::twin;
use portstore::needs::{self, Have, Roms};
use portstore::{State, Store};
use sha2::Digest;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("portstore-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A zip of (name, bytes, deflated); names ending in '/' are folders.
fn zip(path: &Path, members: &[(&str, &[u8], bool)]) {
    zip_made_on(path, members, 3)
}

/// `system` 0 is MS-DOS (Windows), 3 Unix.
fn zip_made_on(path: &Path, members: &[(&str, &[u8], bool)], system: u8) {
    let mut out = Vec::new();
    let mut dir = Vec::new();
    for (name, data, deflate) in members {
        let crc = crc32fast::hash(data);
        let packed = if *deflate {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        } else {
            data.to_vec()
        };
        let method: u16 = if *deflate { 8 } else { 0 };
        let at = out.len() as u32;
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend([20, 0, 0, 0]);
        out.extend(method.to_le_bytes());
        out.extend([0, 0, 0, 0]);
        out.extend(crc.to_le_bytes());
        out.extend((packed.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(&packed);

        dir.extend(0x0201_4b50u32.to_le_bytes());
        dir.extend([20, system, 20, 0, 0, 0]);
        dir.extend(method.to_le_bytes());
        dir.extend([0, 0, 0, 0]);
        dir.extend(crc.to_le_bytes());
        dir.extend((packed.len() as u32).to_le_bytes());
        dir.extend((data.len() as u32).to_le_bytes());
        dir.extend((name.len() as u16).to_le_bytes());
        dir.extend([0, 0, 0, 0, 0, 0, 0, 0]);
        let mode: u32 = if name.ends_with('/') { 0o040755 } else { 0o100644 };
        dir.extend((mode << 16).to_le_bytes());
        dir.extend(at.to_le_bytes());
        dir.extend(name.as_bytes());
    }
    let dir_at = out.len() as u32;
    out.extend(&dir);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend((members.len() as u16).to_le_bytes());
    out.extend((members.len() as u16).to_le_bytes());
    out.extend((dir.len() as u32).to_le_bytes());
    out.extend(dir_at.to_le_bytes());
    out.extend([0, 0]);
    fs::write(path, out).unwrap();
}

fn tar_header(name: &str, size: usize, kind: u8) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[..name.len().min(100)].copy_from_slice(&name.as_bytes()[..name.len().min(100)]);
    h[100..108].copy_from_slice(b"0000644\0");
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    h[156] = kind;
    h[257..263].copy_from_slice(b"ustar\0");
    h[148..156].fill(b' ');
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    h
}

/// A tar.gz of (name, bytes, kind); a name past 100 bytes goes in a GNU long name.
fn tar_gz(path: &Path, members: &[(&str, &[u8], u8)]) {
    let mut t = Vec::new();
    let put = |t: &mut Vec<u8>, data: &[u8]| {
        t.extend(data);
        t.resize(t.len().div_ceil(512) * 512, 0);
    };
    for (name, data, kind) in members {
        if name.len() > 100 {
            let long = format!("{name}\0");
            put(&mut t, &tar_header("././@LongLink", long.len(), b'L'));
            put(&mut t, long.as_bytes());
        }
        put(&mut t, &tar_header(name, data.len(), *kind));
        put(&mut t, data);
    }
    t.extend([0u8; 1024]);
    let mut e = flate2::write::GzEncoder::new(fs::File::create(path).unwrap(), flate2::Compression::default());
    e.write_all(&t).unwrap();
    e.finish().unwrap();
}

fn sha256(path: &Path) -> String {
    hex(&sha2::Sha256::digest(fs::read(path).unwrap()))
}

fn port(id: &str, file: &Path, version: &str) -> Port {
    Port {
        id: id.into(),
        name: "Game".into(),
        version: version.into(),
        file: File {
            url: format!("file://{}", file.display()),
            name: file.file_name().unwrap().to_string_lossy().into(),
            size: fs::metadata(file).unwrap().len(),
            sha256: sha256(file),
            kind: "zip".into(),
        },
        strip: "Game-1.0/".into(),
        take: vec!["App/".into(), "Roms/PORTS/".into()],
        keep: vec!["App/Game/data/".into(), "App/Game/settings.cfg".into()],
        install: Install {
            files: 0,
            bytes: 0,
            roots: vec!["App/Game/".into()],
            marks: vec!["App/Game/launch.sh".into()],
        },
        ..Port::default()
    }
}

fn store(card: &Path) -> Store {
    let home = card.join("App/PortStore");
    fs::create_dir_all(&home).unwrap();
    Store::open(card.to_path_buf(), home)
}

fn files_under(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    if root.exists() {
        walk(root, root, &mut out);
    }
    out.sort();
    out
}

#[test]
fn zip_members_are_read_stored_and_deflated() {
    let dir = temp("zip");
    let z = dir.join("a.zip");
    let big = b"hello hello hello hello ".repeat(400);
    zip(&z, &[("A/", b"", false), ("A/one.txt", b"one", false), ("A/two.bin", &big, true)]);
    let ms = archive::members(&z, "zip").unwrap();
    let names: Vec<_> = ms.iter().map(|m| (m.name.as_str(), m.size, m.dir)).collect();
    assert_eq!(names, [("A/", 0, true), ("A/one.txt", 3, false), ("A/two.bin", big.len() as u64, false)]);
    let mut got = Vec::new();
    archive::extract(&z, "zip", |_| true, |m, r| {
        let mut b = Vec::new();
        r.read_to_end(&mut b)?;
        got.push((m.name.clone(), b));
        Ok(())
    })
    .unwrap();
    assert_eq!(got, [("A/one.txt".to_string(), b"one".to_vec()), ("A/two.bin".to_string(), big)]);
}



#[test]
fn a_damaged_zip_member_is_refused() {
    let dir = temp("crc");
    let z = dir.join("a.zip");
    zip(&z, &[("one.txt", b"original", false)]);
    let mut bytes = fs::read(&z).unwrap();
    let at = bytes.windows(8).position(|w| w == b"original").unwrap();
    bytes[at] = b'X';
    fs::write(&z, bytes).unwrap();
    let err = archive::extract(&z, "zip", |_| true, |_, r| r.read_to_end(&mut Vec::new()).map(|_| ())).unwrap_err();
    assert!(err.to_string().contains("damaged"), "{err}");
}

#[test]
fn unsafe_paths_and_links_are_refused_where_they_would_go_onto_the_card() {
    let card = temp("unsafe");
    let src = temp("unsafe-src");
    let s = store(&card);
    let cancel = AtomicBool::new(false);
    for (i, member) in ["Game-1.0/App/../../etc/passwd", "Game-1.0/App/Game\\x.txt"].iter().enumerate() {
        let z = src.join(format!("Game-{i}.zip"));
        zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false), (member, b"x", false)]);
        let err = s.install(&port("game", &z, "1.0"), &cancel, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("unsafe"), "{err}");
        assert!(!card.join("App/Game").exists(), "nothing is placed");
    }
    // Elsewhere in the archive (not taken), the same is left alone.
    let z = src.join("Game-ok.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false), ("Game-1.0/licenses\\a.txt", b"x", false)]);
    s.install(&port("game", &z, "1.0"), &cancel, &mut |_| {}).unwrap();

    let t = src.join("a.tar.gz");
    tar_gz(&t, &[("App/X/link", b"", b'2'), ("App/X/a", b"a", b'0')]);
    let ms = archive::members(&t, "tar.gz").unwrap();
    assert!(ms[0].link && !ms[1].link);
}

#[test]
fn backslashes_of_a_zip_made_on_windows_are_folders() {
    let dir = temp("dos");
    let z = dir.join("a.zip");
    zip_made_on(&z, &[("Roms\\PORTS\\Games\\X\\a.txt", b"a", false)], 0);
    let ms = archive::members(&z, "zip").unwrap();
    assert_eq!(ms[0].name, "Roms/PORTS/Games/X/a.txt");
}

#[test]
fn an_app_zipped_without_its_folder_goes_into_one() {
    let card = temp("into");
    let src = temp("into-src");
    let s = store(&card);
    let z = src.join("Petals.zip");
    zip(&z, &[("launch.sh", b"x", false), ("config.json", b"{}", false)]);
    let mut p = port("petals", &z, "1.0");
    p.categories = vec!["app".into()];
    p.strip = String::new();
    p.into = "App/Petals/".into();
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(files_under(&card.join("App/Petals")), ["config.json", "launch.sh"]);
}

#[test]
fn tar_gz_with_long_names_is_read() {
    let dir = temp("tar");
    let t = dir.join("a.tar.gz");
    let long = format!("Roms/PORTS/Games/{}/file.txt", "x".repeat(120));
    tar_gz(&t, &[("Roms/", b"", b'5'), ("./Roms/a.txt", b"aaa", b'0'), (&long, b"long", b'0')]);
    let ms = archive::members(&t, "tar.gz").unwrap();
    let names: Vec<_> = ms.iter().map(|m| m.name.clone()).collect();
    assert_eq!(names, ["Roms/".to_string(), "Roms/a.txt".into(), long.clone()]);
    let mut got = Vec::new();
    archive::extract(&t, "tar.gz", |_| true, |m, r| {
        let mut s = String::new();
        r.read_to_string(&mut s)?;
        got.push((m.name.clone(), s));
        Ok(())
    })
    .unwrap();
    assert_eq!(got[1], (long, "long".to_string()));
}

#[test]
fn twins_are_the_two_names_of_a_shortcut() {
    assert_eq!(twin("Roms/PORTS/Shortcuts/RPG/X.port").as_deref(), Some("Roms/PORTS/Shortcuts/RPG/X.notfound"));
    assert_eq!(twin("Roms/PORTS/Shortcuts/X.notfound").as_deref(), Some("Roms/PORTS/Shortcuts/X.port"));
    assert_eq!(twin("App/X/launch.port"), None);
}

#[test]
fn install_update_and_remove_leave_the_users_files() {
    let card = temp("card");
    let src = temp("card-src");
    let s = store(&card);
    fs::create_dir_all(card.join("App/Other")).unwrap();
    fs::write(card.join("App/Other/keep.txt"), "other app").unwrap();

    let v1 = src.join("Game-1.0.zip");
    zip(
        &v1,
        &[
            ("Game-1.0/App/Game/launch.sh", b"v1", false),
            ("Game-1.0/App/Game/old.bin", b"old", true),
            ("Game-1.0/App/Game/settings.cfg", b"defaults", false),
            ("Game-1.0/Roms/PORTS/Shortcuts/Game.port", b"GameDataFile=\"launch.sh\"", false),
            ("Game-1.0/Tools/convert.exe", b"MZ", false),
        ],
    );
    let p1 = port("game", &v1, "1.0");
    assert_eq!(s.state(&p1), State::Absent);
    let cancel = AtomicBool::new(false);
    let rec = s.install(&p1, &cancel, &mut |_| {}).unwrap();
    assert_eq!(rec.files.len(), 5, "{:?}", rec.files);
    assert!(rec.shortcuts);
    assert_eq!(s.state(&p1), State::Installed("1.0".into()));
    assert!(!card.join("Tools").exists(), "files for a PC stay out");
    assert_eq!(fs::read_to_string(card.join("App/Game/launch.sh")).unwrap(), "v1");
    assert!(card.join("Roms/MiyooPorts/.ports/Shortcuts/Game.port").exists(), "the release's shortcut, kept hidden");
    assert!(!card.join("Roms/PORTS/Shortcuts/Game.port").exists(), "not in Ports");
    assert!(card.join("Roms/MiyooPorts/Game.sh").exists(), "in MiyooPorts");
    assert!(card.join("Emu/MiyooPorts/config.json").exists());
    assert!(!card.join("App/PortStore/cache/Game-1.0.zip").exists(), "the download is not kept");

    // The user plays: saves, changed settings.
    fs::create_dir_all(card.join("App/Game/data")).unwrap();
    fs::write(card.join("App/Game/data/save.dat"), "save").unwrap();
    fs::write(card.join("App/Game/settings.cfg"), "mine").unwrap();
    fs::write(card.join("App/Game/user-made.txt"), "notes").unwrap();

    let v2 = src.join("Game-2.0.zip");
    zip(
        &v2,
        &[
            ("Game-1.0/App/Game/launch.sh", b"v2", false),
            ("Game-1.0/App/Game/new.bin", b"new", true),
            ("Game-1.0/App/Game/settings.cfg", b"defaults 2", false),
            ("Game-1.0/Roms/PORTS/Shortcuts/Game.port", b"GameDataFile=\"launch.sh\"", false),
        ],
    );
    let p2 = port("game", &v2, "2.0");
    assert_eq!(s.state(&p2), State::Update("1.0".into()));
    s.install(&p2, &cancel, &mut |_| {}).unwrap();
    assert_eq!(fs::read_to_string(card.join("App/Game/launch.sh")).unwrap(), "v2");
    assert!(!card.join("App/Game/old.bin").exists(), "a file the new version dropped goes");
    assert!(card.join("App/Game/new.bin").exists());
    assert_eq!(fs::read_to_string(card.join("App/Game/settings.cfg")).unwrap(), "mine");
    assert_eq!(fs::read_to_string(card.join("App/Game/data/save.dat")).unwrap(), "save");
    assert!(card.join("Roms/MiyooPorts/.ports/Shortcuts/Game.port").exists());

    s.remove("game", Some(&p2)).unwrap();
    assert_eq!(s.state(&p2), State::Absent);
    let left = files_under(&card.join("App"));
    assert_eq!(
        left,
        [
            "Game/data/save.dat",
            "Game/settings.cfg",
            "Game/user-made.txt",
            "Other/keep.txt",
        ]
    );
    assert!(!card.join("Roms/MiyooPorts/.ports/Shortcuts/Game.port").exists());
    assert!(!card.join("Roms/MiyooPorts/Game.sh").exists());
    assert!(card.join("Roms/MiyooPorts").exists(), "MiyooPorts' own folder stays");
}

#[test]
fn shortcuts_get_the_folder_onions_import_looks_from() {
    // miyoo64's shortcuts say GameDir="../../../App/<Game>", which ~Import ports
    // reads from Roms/PORTS/Games: without that folder the port shows as not found.
    let card = temp("games-dir");
    let src = temp("games-dir-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(
        &z,
        &[
            ("Game-1.0/App/Game/launch.sh", b"x", false),
            ("Game-1.0/App/Game/saves/", b"", false),
            ("Game-1.0/Roms/PORTS/Shortcuts/Game.port", b"GameDir=\"../../../App/Game\"", false),
            ("Game-1.0/Tools/", b"", false),
        ],
    );
    s.install(&port("game", &z, "1.0"), &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(card.join("Roms/PORTS/Games").is_dir());
    assert!(card.join("Roms/MiyooPorts/.ports/Shortcuts/Game.port").exists());
    assert!(card.join("Roms/PORTS/Games/../../../App/Game/launch.sh").exists());
    assert!(card.join("App/Game/saves").is_dir(), "the archive's empty folders too");
    assert!(!card.join("Tools").exists());

    // A card left without it by an earlier Port Store is put right on start.
    fs::remove_dir(card.join("Roms/PORTS/Games")).unwrap();
    let _ = fs::remove_file(card.join("App/PortStore/cache/import"));
    s.repair(&Index::default());
    assert!(card.join("Roms/PORTS/Games").is_dir());
    assert!(card.join("App/PortStore/cache/import").exists(), "and ~Import ports runs again");
}

#[test]
fn a_removed_port_takes_its_empty_folders_along() {
    let card = temp("card-empty");
    let src = temp("card-empty-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/lib/a.so", b"so", false), ("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let p = port("game", &z, "1.0");
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    s.remove("game", Some(&p)).unwrap();
    assert!(!card.join("App/Game").exists());
    assert!(card.join("App").exists());
}

#[test]
fn a_file_two_ports_share_stays_while_one_has_it() {
    let card = temp("shared");
    let src = temp("shared-src");
    let s = store(&card);
    let cancel = AtomicBool::new(false);
    let a = src.join("Game-a.zip");
    zip(&a, &[("Game-1.0/App/Game/launch.sh", b"a", false), ("Game-1.0/App/Common/lib.so", b"lib", false)]);
    let b = src.join("Game-b.zip");
    zip(&b, &[("Game-1.0/App/Other/launch.sh", b"b", false), ("Game-1.0/App/Common/lib.so", b"lib", false)]);
    let (pa, pb) = (port("a", &a, "1"), port("b", &b, "1"));
    s.install(&pa, &cancel, &mut |_| {}).unwrap();
    s.install(&pb, &cancel, &mut |_| {}).unwrap();
    s.remove("a", Some(&pa)).unwrap();
    assert!(card.join("App/Common/lib.so").exists());
    s.remove("b", Some(&pb)).unwrap();
    assert!(!card.join("App/Common").exists());
}

#[test]
fn a_file_that_is_not_the_catalogs_is_refused() {
    let card = temp("card-sha");
    let src = temp("card-sha-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut p = port("game", &z, "1.0");
    p.file.sha256 = "0".repeat(64);
    let err = s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("checksum"), "{err}");
    assert!(!card.join("App/Game").exists());
}

#[test]
fn a_file_put_in_downloads_is_used_and_kept() {
    let card = temp("card-offline");
    let src = temp("card-offline-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut p = port("game", &z, "1.0");
    let by_hand = card.join("App/PortStore/downloads/Game-1.0.zip");
    fs::create_dir_all(by_hand.parent().unwrap()).unwrap();
    fs::copy(&z, &by_hand).unwrap();
    p.file.url = "https://example.invalid/nothing-here.zip".into();
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(card.join("App/Game/launch.sh").exists());
    assert!(by_hand.exists());
}

#[test]
fn a_port_put_on_the_card_by_hand_is_told() {
    let card = temp("card-hand");
    let s = store(&card);
    let p = Port {
        id: "x".into(),
        install: Install {
            files: 1,
            bytes: 1,
            roots: vec!["App/Game/".into()],
            marks: vec!["App/Game/launch.sh".into()],
        },
        ..Port::default()
    };
    assert_eq!(s.state(&p), State::Absent);
    fs::create_dir_all(card.join("App/Game")).unwrap();
    assert_eq!(s.state(&p), State::Absent, "an empty folder is not the port");
    fs::write(card.join("App/Game/launch.sh"), "").unwrap();
    assert_eq!(s.state(&p), State::ByHand);
}

#[test]
fn n64_roms_are_known_in_any_byte_order() {
    let dir = temp("roms");
    let mut z64 = vec![0x80, 0x37, 0x12, 0x40];
    z64.resize(0x20, 0);
    z64.extend(b"MARIOKART64         ");
    z64.extend((0..5000u32).map(|i| (i * 7) as u8));
    let v64: Vec<u8> = z64.chunks(2).flat_map(|w| [w[1], w[0]]).collect();
    let n64: Vec<u8> = z64.chunks(4).flat_map(|w| [w[3], w[2], w[1], w[0]]).collect();
    fs::write(dir.join("a.z64"), &z64).unwrap();
    fs::write(dir.join("b.v64"), &v64).unwrap();
    fs::write(dir.join("c.n64"), &n64).unwrap();
    let want = hex(&<sha1::Sha1 as sha1::Digest>::digest(&z64));
    for f in ["a.z64", "b.v64", "c.n64"] {
        let (sha, title) = needs::n64_sha1(&dir.join(f)).unwrap();
        assert_eq!(sha, want, "{f}");
        assert_eq!(title, "MARIOKART64");
    }
}

#[test]
fn needs_are_checked_on_the_card() {
    let card = temp("needs");
    let home = card.join("App/PortStore");
    fs::create_dir_all(card.join("Roms/N64")).unwrap();
    let mut rom = vec![0x80, 0x37, 0x12, 0x40];
    rom.resize(0x20, 0);
    rom.extend(b"MARIOKART64         ");
    rom.extend([1u8; 100]);
    let sha = hex(&<sha1::Sha1 as sha1::Digest>::digest(&rom));
    let need = |sha: &str| Need {
        what: "ROM".into(),
        sha1: sha.into(),
        search: vec!["Roms/N64".into(), "App/MarioKart64".into()],
        n64: true,
        title: "MARIOKART64".into(),
        ..Need::default()
    };
    let data = Need {
        what: "data".into(),
        check: vec!["Roms/PORTS/Games/X/data.win".into()],
        ..Need::default()
    };
    let p = Port { needs: vec![need(&sha), data.clone()], ..Port::default() };
    let mut roms = Roms::open(&home);
    let check = |roms: &mut Roms, p: &Port| needs::check(&card, p, roms, &mut |_| {});
    assert_eq!(check(&mut roms, &p), [Have::Missing, Have::Missing]);

    fs::write(card.join("Roms/N64/Mario Kart 64 (E).z64"), {
        let mut other = rom.clone();
        other[200 - 100] = 9;
        other
    })
    .unwrap();
    assert_eq!(
        check(&mut roms, &p)[0],
        Have::WrongVersion("Roms/N64/Mario Kart 64 (E).z64".into())
    );
    fs::write(card.join("Roms/N64/mk64.z64"), &rom).unwrap();
    fs::create_dir_all(card.join("Roms/PORTS/Games/X")).unwrap();
    fs::write(card.join("Roms/PORTS/Games/X/data.win"), "").unwrap();
    assert_eq!(
        check(&mut roms, &p),
        [Have::Found("Roms/N64/mk64.z64".into()), Have::Found("Roms/PORTS/Games/X/data.win".into())]
    );
    assert!(home.join("cache/roms.json").exists());
    // As ~Import ports: the data file in any case, a folder down.
    let deep = Port {
        needs: vec![Need { check: vec!["Roms/PORTS/Games/HL/liblist.gam".into()], ..Need::default() }],
        ..Port::default()
    };
    assert_eq!(check(&mut roms, &deep), [Have::Missing]);
    fs::create_dir_all(card.join("Roms/PORTS/Games/HL/valve")).unwrap();
    fs::write(card.join("Roms/PORTS/Games/HL/valve/LIBLIST.GAM"), "").unwrap();
    assert_eq!(check(&mut roms, &deep), [Have::Found("Roms/PORTS/Games/HL/valve/LIBLIST.GAM".into())]);
    // Another game's ROM is not "another version".
    let other = Port { needs: vec![Need { title: "SUPER MARIO 64".into(), ..need(&"0".repeat(40)) }], ..Port::default() };
    assert_eq!(check(&mut roms, &other), [Have::Missing]);
}

#[test]
fn the_newest_catalog_at_hand_is_used() {
    let card = temp("index");
    let s = store(&card);
    let write = |path: PathBuf, generated: &str| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let index = Index { format: 1, generated: generated.into(), ..Index::default() };
        fs::write(path, serde_json::to_string(&index).unwrap()).unwrap();
    };
    write(card.join("App/PortStore/index.json"), "2026-10-20T00:00:00Z");
    assert_eq!(s.load_index().generated, "2026-10-20T00:00:00Z");
    write(card.join("App/PortStore/cache/index.json"), "2026-10-21T00:00:00Z");
    assert_eq!(s.load_index().generated, "2026-10-21T00:00:00Z");
    write(card.join("App/PortStore/index.json"), "2026-10-22T00:00:00Z");
    assert_eq!(s.load_index().generated, "2026-10-22T00:00:00Z");
    // Brought on a computer, for a Mini without Wi-Fi.
    write(card.join("App/PortStore/downloads/index.json"), "2026-10-23T00:00:00Z");
    assert_eq!(s.load_index().generated, "2026-10-23T00:00:00Z");
}

#[test]
fn a_game_released_as_an_app_is_in_miyooports_not_apps() {
    let card = temp("made-shortcut");
    let src = temp("made-shortcut-src");
    let s = store(&card);
    let z = src.join("petals.zip");
    zip(
        &z,
        &[
            ("launch.sh", b"./petals", false),
            ("config.json", br#"{"label": "Petals", "launch": "launch.sh"}"#, false),
            ("icon.png", b"PNG", false),
        ],
    );
    let mut p = port("petals", &z, "1.0");
    p.name = "Petals: Around the Rose".into();
    p.strip = String::new();
    p.into = "App/Petals/".into();
    let rec = s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(!card.join("App/Petals/config.json").exists(), "not in Apps");
    assert_eq!(rec.apps.len(), 1);
    let sh = fs::read_to_string(card.join("Roms/MiyooPorts/Petals Around the Rose.sh")).unwrap();
    assert!(sh.starts_with("#!/bin/sh\n"), "{sh}");
    assert!(sh.contains("cd \"$card\"/'App/Petals' || exit 1\n"), "{sh}");
    assert!(sh.contains("exec sh ./'launch.sh'\n"), "{sh}");
    assert!(sh.contains("\n    export LD_PRELOAD="), "{sh}");
    assert!(rec.shortcuts);
    for f in ["Emu/MiyooPorts/config.json", "Emu/MiyooPorts/launch.sh", "Icons/Default/miyooports.png"] {
        assert!(card.join(f).exists(), "{f}");
    }
    s.remove("petals", Some(&p)).unwrap();
    assert!(!card.join("Roms/MiyooPorts/Petals Around the Rose.sh").exists());
}

#[test]
fn a_launcher_says_what_is_missing_and_runs_the_game_once_it_is_there() {
    let card = temp("launcher-run");
    let src = temp("launcher-run-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"echo started > started.txt", false)]);
    let mut p = port("game", &z, "1.0");
    p.shortcut = Some(Shortcut { launch: "App/Game/launch.sh".into(), ..Default::default() });
    p.needs = vec![Need {
        what: "The game's data".into(),
        hint: "Copy data.win into App/Game.".into(),
        check: vec!["App/Game/data.win".into()],
        ..Need::default()
    }];
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    let sh = fs::read_to_string(card.join("Roms/MiyooPorts/Game.sh")).unwrap();
    // Run it on this card: infoPanel as a stand-in that writes what it says.
    let sh = sh.replace("card=/mnt/SDCARD", &format!("card='{}'", card.display()));
    fs::create_dir_all(card.join(".tmp_update/bin")).unwrap();
    script(&card.join(".tmp_update/bin"), "infoPanel", r#"echo "$@" > "$(dirname "$0")/said.txt""#);
    let run = |sh: &str| std::process::Command::new("sh").arg("-c").arg(sh).status().unwrap();
    assert!(run(&sh).success());
    let said = fs::read_to_string(card.join(".tmp_update/bin/said.txt")).unwrap();
    assert!(said.contains("your files are missing") && said.contains("Copy data.win"), "{said}");
    assert!(!card.join("App/Game/started.txt").exists());
    // In any case, a folder down, as OnionOS's ~Import ports finds it.
    fs::create_dir_all(card.join("App/Game/sub")).unwrap();
    fs::write(card.join("App/Game/sub/DATA.WIN"), "").unwrap();
    assert!(run(&sh).success());
    assert!(card.join("App/Game/started.txt").exists());
}

#[test]
fn an_update_takes_away_a_file_now_skipped() {
    let card = temp("skip-update");
    let src = temp("skip-update-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false), ("Game-1.0/App/Game/config.json", b"{}", false)]);
    let mut p1 = port("game", &z, "1.0");
    p1.categories = vec!["app".into()];
    s.install(&p1, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(card.join("App/Game/config.json").exists());
    let mut p2 = port("game", &z, "1.0b");
    p2.categories = vec!["app".into()];
    p2.skip = vec!["App/Game/config.json".into()];
    s.install(&p2, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(!card.join("App/Game/config.json").exists(), "the game leaves Apps on update");
    assert!(card.join("App/Game/launch.sh").exists());
}

#[test]
fn ports_installed_before_miyooports_move_into_it_on_start() {
    let card = temp("conform");
    let s = store(&card);
    // As Port Store 0.3 left a game: its shortcut in Ports (renamed by ~Import
    // ports), its picture there, its entry in Apps.
    for (f, text) in [
        ("App/Game/launch.sh", "x"),
        ("App/Game/config.json", r#"{"label": "Game", "launch": "launch.sh"}"#),
        ("Roms/PORTS/Shortcuts/Game.notfound", "GameDir=\"../../../App/Game\""),
        ("Roms/PORTS/Imgs/Game.png", "PNG"),
    ] {
        fs::create_dir_all(card.join(f).parent().unwrap()).unwrap();
        fs::write(card.join(f), text).unwrap();
    }
    let rec = serde_json::json!({
        "id": "game", "version": "1.0", "shortcuts": true,
        "files": ["App/Game/launch.sh", "App/Game/config.json", "Roms/PORTS/Shortcuts/Game.port", "Roms/PORTS/Imgs/Game.png"]
    });
    fs::create_dir_all(card.join("App/PortStore/installed")).unwrap();
    fs::write(card.join("App/PortStore/installed/game.json"), rec.to_string()).unwrap();
    let p = Port { id: "game".into(), name: "Game".into(), version: "1.0".into(), ..Port::default() };
    s.repair(&Index { format: 1, ports: vec![p.clone()], ..Index::default() });
    assert!(!card.join("Roms/PORTS/Shortcuts/Game.notfound").exists(), "out of Ports");
    assert!(!card.join("App/Game/config.json").exists(), "out of Apps");
    assert!(card.join("Roms/MiyooPorts/.ports/Shortcuts/Game.port").exists());
    assert_eq!(fs::read(card.join("Roms/MiyooPorts/Imgs/Game.png")).unwrap(), b"PNG", "its picture");
    let sh = fs::read_to_string(card.join("Roms/MiyooPorts/Game.sh")).unwrap();
    assert!(sh.contains("port=\"$card\"/'Roms/MiyooPorts/.ports/Shortcuts/Game.port'"), "{sh}");
    assert_eq!(s.state(&p), State::Installed("1.0".into()), "the same version still");
    // Once only: a second start changes nothing.
    let before = fs::read_to_string(card.join("App/PortStore/installed/game.json")).unwrap();
    s.repair(&Index { format: 1, ports: vec![p.clone()], ..Index::default() });
    assert_eq!(fs::read_to_string(card.join("App/PortStore/installed/game.json")).unwrap(), before);
    s.remove("game", Some(&p)).unwrap();
    assert_eq!(files_under(&card.join("Roms/MiyooPorts")), Vec::<String>::new());
}

#[test]
fn ports_collection_is_added_as_package_manager_does() {
    let card = temp("ports-collection");
    let s = store(&card);
    assert!(s.add_ports_collection().is_err(), "no package, nothing to add");
    let pkg = card.join("App/PackageManager/data/Emu/Ports Collection");
    fs::create_dir_all(pkg.join("Emu/PORTS")).unwrap();
    fs::create_dir_all(pkg.join("Roms/PORTS/Shortcuts/Action")).unwrap();
    fs::write(pkg.join("Emu/PORTS/config.json"), "{}").unwrap();
    fs::write(pkg.join("Emu/PORTS/install.sh"), "touch ran-install\n").unwrap();
    fs::write(pkg.join("Roms/PORTS/Shortcuts/Action/Gong.port"), "x").unwrap();
    assert!(!s.has_ports_collection());
    s.add_ports_collection().unwrap();
    assert!(s.has_ports_collection());
    assert!(card.join("Roms/PORTS/Shortcuts/Action/Gong.port").exists());
    assert!(pkg.join("Emu/PORTS/ran-install").exists(), "install.sh ran where Package Manager runs it");
    assert!(card.join("App/PortStore/cache/import").exists());
}

#[test]
fn a_release_laid_out_otherwise_is_mapped_onto_the_card() {
    let card = temp("map");
    let src = temp("map-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(
        &z,
        &[
            ("Game-1.0/Game.port", b"GameDir=\"Game\"", false),
            ("Game-1.0/Games/Game/game", b"elf", false),
            ("Game-1.0/README.txt", b"read me", false),
        ],
    );
    let mut p = port("game", &z, "1.0");
    p.map = vec![
        ["Game.port".into(), "Roms/PORTS/Shortcuts/Game.port".into()],
        ["Games/".into(), "Roms/PORTS/Games/".into()],
    ];
    let rec = s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(card.join("Roms/MiyooPorts/.ports/Shortcuts/Game.port").exists());
    assert!(card.join("Roms/PORTS/Games/Game/game").exists());
    assert!(!card.join("README.txt").exists());
    assert_eq!(rec.files.len(), 3, "the two, and the launcher: {:?}", rec.files);
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn without_a_connection_the_file_is_listed_for_a_computer() {
    let card = temp("offline-list");
    let src = temp("offline-list-src");
    let mut s = store(&card);
    // curl as on a Mini without Wi-Fi: "could not resolve host".
    s.fetcher.curl = script(&src, "curl", "exit 6");
    s.index_url = "https://example.org/index.json".into();
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut a = port("a", &z, "1.0");
    a.file.url = "https://example.org/a/Game-1.0.zip".into();
    let err = s.install(&a, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotConnected);
    assert!(err.to_string().contains("to-download.txt"), "{err}");
    let mut b = a.clone();
    b.id = "b".into();
    b.file.name = "Other-2.0.zip".into();
    b.file.url = "https://example.org/b/Other-2.0.zip".into();
    let _ = s.install(&b, &AtomicBool::new(false), &mut |_| {});
    let list = fs::read_to_string(card.join("App/PortStore/downloads/to-download.txt")).unwrap();
    assert!(list.contains("index.json:\n  https://example.org/index.json\n"), "{list}");
    assert!(list.contains("File: Game-1.0.zip\n  https://example.org/a/Game-1.0.zip\n"), "{list}");
    assert!(list.contains("File: Other-2.0.zip\n"), "both, the first kept: {list}");
    // Brought: off the list, and installed from downloads/.
    fs::copy(&z, card.join("App/PortStore/downloads/Game-1.0.zip")).unwrap();
    s.install(&a, &AtomicBool::new(false), &mut |_| {}).unwrap();
    s.list_for_computer(&[]);
    let list = fs::read_to_string(card.join("App/PortStore/downloads/to-download.txt")).unwrap();
    assert!(!list.contains("Game-1.0.zip"), "{list}");
    assert!(list.contains("Other-2.0.zip"), "{list}");
}

#[test]
fn a_7z_release_is_unpacked_by_7zip_and_installed() {
    let card = temp("7z");
    let src = temp("7z-src");
    let s = store(&card);
    // A stand-in for 7-Zip: "x -y -bd -o<dir> <file>", here a zip unpacked by unzip.
    let seven = script(&src, "7z", r#"out=""; for a; do case "$a" in -o*) out="${a#-o}";; esac; file="$a"; done; unzip -q -o "$file" -d "$out""#);
    std::env::set_var("PORTSTORE_7Z", &seven);
    let z = src.join("OpenRCT2mini.7z");
    zip(&z, &[("Roms/PORTS/Games/RCT/rct", b"elf", false), ("Roms/PORTS/Shortcuts/RCT.port", b"x", false), ("README", b"r", false)]);
    let mut p = port("rct", &z, "0.3.0");
    p.file.kind = "7z".into();
    p.strip = String::new();
    p.install.bytes = 4;
    let rec = s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(rec.files.len(), 3, "{:?}", rec.files);
    assert!(card.join("Roms/MiyooPorts/Game.sh").exists());
    assert!(card.join("Roms/PORTS/Games/RCT/rct").exists());
    assert!(!card.join("README").exists());
    assert!(!card.join("App/PortStore/cache/7z-rct").exists(), "the unpacked copy goes");
}

#[test]
fn a_catalog_with_an_older_release_than_the_one_installed_is_no_update() {
    let card = temp("older");
    let src = temp("older-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut new = port("game", &z, "v0.3.0");
    new.date = "2026-10-21".into();
    s.install(&new, &AtomicBool::new(false), &mut |_| {}).unwrap();
    let mut old = new.clone();
    old.version = "v0.2.0".into();
    old.date = "2026-10-20".into();
    assert_eq!(s.state(&old), State::Installed("v0.3.0".into()));
    let mut newer = new.clone();
    newer.version = "v0.4.0".into();
    newer.date = "2026-10-25".into();
    assert_eq!(s.state(&newer), State::Update("v0.3.0".into()));
}

#[test]
fn versions_are_compared_by_their_numbers_then_by_date() {
    use portstore::newer;
    assert!(newer("v0.3.0", "", "v0.2.0", ""));
    assert!(!newer("v0.2.0", "2026-10-20", "v0.3.0", "2026-10-20"), "a catalog behind the device");
    assert!(newer("v0.10.0", "", "v0.9.1", ""));
    assert!(newer("onion-v3", "", "onion-v2", ""));
    assert!(newer("initial-release", "2026-11-01", "initial", "2026-10-01"));
    assert!(!newer("initial-release", "2026-09-01", "initial", "2026-10-01"));
    assert!(newer("nightly-b", "", "nightly-a", ""), "another, nothing to go by");
}

#[test]
fn port_store_knows_its_own_version_without_an_install_record() {
    let card = temp("self");
    let s = store(&card);
    let me = format!("v{}", env!("CARGO_PKG_VERSION"));
    let mut p = Port { id: portstore::SELF_ID.into(), version: "v999.0.0".into(), ..Port::default() };
    assert_eq!(s.state(&p), State::Update(me.clone()));
    p.version = me.clone();
    assert_eq!(s.state(&p), State::Installed(me.clone()));
    p.version = "v0.0.1".into();
    assert_eq!(s.state(&p), State::Installed(me), "an older catalog is no update");
}

#[test]
fn the_number_of_updates_shows_under_port_stores_name_in_apps() {
    let card = temp("note");
    let s = store(&card);
    let config = card.join("App/PortStore/config.json");
    // As OnionOS's icon packs leave it: their own keys, which stay.
    fs::write(&config, r#"{"label": "Port Store", "icon": "/mnt/SDCARD/Icons/x.png", "launch": "launch.sh", "description": "Install and update ports"}"#).unwrap();
    s.note_updates(3);
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(json["description"], "Install and update ports · 3 updates");
    assert_eq!(json["icon"], "/mnt/SDCARD/Icons/x.png");
    s.note_updates(1);
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(json["description"], "Install and update ports · 1 update");
    s.note_updates(0);
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(json["description"], "Install and update ports");
}

#[test]
fn game_files_brought_into_import_find_their_place() {
    let card = temp("import");
    let home = card.join("App/PortStore");
    let s = store(&card);
    let put = |f: &str| {
        let p = home.join("import").join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, f).unwrap();
    };
    // An Undertale folder named as it came, RCT2 with its folders, Half-Life's valve
    // inside its own folder, a ROM loose.
    put("Undertale (Windows)/data.win");
    put("Undertale (Windows)/mus_x.ogg");
    put("RollerCoaster Tycoon 2/Data/G1.DAT");
    put("RollerCoaster Tycoon 2/ObjData/ride.dat");
    put("Half-Life/valve/liblist.gam");
    let mut rom = vec![0x80, 0x37, 0x12, 0x40];
    rom.resize(0x40, 7);
    fs::write(home.join("import/mk64.z64"), &rom).unwrap();
    let need = |check: &str| Need { what: "files".into(), check: vec![check.into()], ..Need::default() };
    let p = Port {
        needs: vec![
            need("Roms/PORTS/Games/Undertale/data.win"),
            need("Roms/PORTS/Games/OpenRCT2mini/rct2/Data/g1.dat"),
            need("Roms/PORTS/Games/Half-Life/valve/liblist.gam"),
            Need {
                what: "ROM".into(),
                sha1: hex(&<sha1::Sha1 as sha1::Digest>::digest(&rom)),
                search: vec!["Roms/N64".into()],
                ..Need::default()
            },
            need("Roms/PORTS/Games/Other/nothing.here"),
        ],
        ..Port::default()
    };
    let mut roms = Roms::open(&home);
    let have = needs::check(&card, &p, &mut roms, &mut |_| {});
    let found = needs::importable(&card, &home, &p, &have, &mut roms);
    let placed: Vec<Option<(String, String)>> =
        found.iter().map(|f| f.as_ref().map(|f| (f.shown.clone(), f.to.clone()))).collect();
    assert_eq!(
        placed,
        [
            Some(("import/Undertale (Windows)".into(), "Roms/PORTS/Games/Undertale".into())),
            Some(("import/RollerCoaster Tycoon 2".into(), "Roms/PORTS/Games/OpenRCT2mini/rct2".into())),
            Some(("import/Half-Life".into(), "Roms/PORTS/Games/Half-Life".into())),
            Some(("import/mk64.z64".into(), "Roms/N64/mk64.z64".into())),
            None,
        ]
    );
    for f in found.iter().flatten() {
        needs::place(&card, f).unwrap();
    }
    assert!(card.join("Roms/PORTS/Games/Undertale/mus_x.ogg").exists());
    assert!(card.join("Roms/PORTS/Games/OpenRCT2mini/rct2/ObjData/ride.dat").exists());
    assert!(card.join("Roms/PORTS/Games/Half-Life/valve/liblist.gam").exists());
    assert!(card.join("Roms/N64/mk64.z64").exists());
    let have = needs::check(&card, &p, &mut roms, &mut |_| {});
    assert!(matches!(have[..4], [Have::Found(_), Have::Found(_), Have::Found(_), Have::Found(_)]), "{have:?}");
    assert_eq!(files_under(&home.join("import")), Vec::<String>::new(), "import/ left empty");
    let _ = s;
}

// ---- review fixes (1.0.0) ----

/// Set the compression method of `name` in a zip's central directory.
fn patch_method(path: &Path, name: &str, method: u16) {
    let mut b = fs::read(path).unwrap();
    let mut at = 0;
    while let Some(i) = b[at..].windows(4).position(|w| w == [0x50, 0x4b, 0x01, 0x02]) {
        let h = at + i;
        let n = u16::from_le_bytes([b[h + 28], b[h + 29]]) as usize;
        if &b[h + 46..h + 46 + n] == name.as_bytes() {
            b[h + 10..h + 12].copy_from_slice(&method.to_le_bytes());
        }
        at = h + 4;
    }
    fs::write(path, b).unwrap();
}

#[test]
fn port_store_never_removes_itself() {
    let card = temp("self-remove");
    let s = store(&card);
    fs::create_dir_all(card.join("App/PortStore/installed")).unwrap();
    fs::write(card.join("App/PortStore/portstore"), "elf").unwrap();
    fs::write(
        card.join("App/PortStore/installed/portstore.json"),
        r#"{"id":"portstore","version":"v1.0.0","files":["App/PortStore/portstore"]}"#,
    )
    .unwrap();
    assert!(s.remove(portstore::SELF_ID, None).is_err());
    assert!(card.join("App/PortStore/portstore").exists());
}

#[test]
fn a_catalog_entry_port_store_cannot_read_costs_only_itself() {
    let good = r#"{"id":"good","name":"Good","version":"1","file":{"url":"u","name":"g.zip","size":1,"sha256":"x","kind":"zip"},"take":["App/"],"install":{"files":1,"bytes":1}}"#;
    let typed_wrong = r#"{"id":"bad","name":"Bad","categories":"game","version":"1","file":{"url":"u","name":"b.zip","size":1,"sha256":"x","kind":"zip"},"take":["App/"],"install":{"files":1,"bytes":1}}"#;
    let off_card = r#"{"id":"evil","name":"Evil","version":"1","into":"/etc/","file":{"url":"u","name":"e.zip","size":1,"sha256":"x","kind":"zip"},"take":[],"install":{"files":1,"bytes":1}}"#;
    let climbing = r#"{"id":"../x","name":"X","version":"1","file":{"url":"u","name":"x.zip","size":1,"sha256":"x","kind":"zip"},"take":["App/"],"install":{"files":1,"bytes":1}}"#;
    let text = format!(
        r#"{{"format":1,"generated":"g","ports":[{good},{typed_wrong},{off_card},{climbing}],"manual":[{{"name":"M","categories":"game"}},{{"name":"N"}}]}}"#
    );
    let index = Index::parse(&text).unwrap();
    assert_eq!(index.ports.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["good"]);
    assert_eq!(index.manual.len(), 1);
}

#[test]
fn an_update_that_changes_only_case_keeps_its_file() {
    let card = temp("case");
    let src = temp("case-src");
    let s = store(&card);
    let z1 = src.join("v1.zip");
    zip(&z1, &[("Game-1.0/App/Game/Data/x.bin", b"1", false), ("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let z2 = src.join("v2.zip");
    zip(&z2, &[("Game-1.0/App/Game/data/x.bin", b"2", false), ("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let (mut p1, mut p2) = (port("game", &z1, "1"), port("game", &z2, "2"));
    p1.categories = vec!["app".into()];
    p2.categories = vec!["app".into()];
    s.install(&p1, &AtomicBool::new(false), &mut |_| {}).unwrap();
    s.install(&p2, &AtomicBool::new(false), &mut |_| {}).unwrap();
    // On FAT32 the two are one file: the update must not remove "Data/x.bin".
    assert!(card.join("App/Game/Data/x.bin").exists(), "not removed as gone");
}

#[test]
fn members_landing_on_one_path_are_one_file() {
    let card = temp("dupes");
    let src = temp("dupes-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(
        &z,
        &[
            ("Game-1.0/App/Game/a.txt", b"first", false),
            ("Game-1.0/App/Game/A.TXT", b"second", false),
            ("Game-1.0/App/Game/a.txt", b"third", false),
        ],
    );
    let mut p = port("game", &z, "1");
    p.categories = vec!["app".into()];
    let rec = s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(rec.files, ["App/Game/a.txt"]);
    assert_eq!(fs::read_to_string(card.join("App/Game/a.txt")).unwrap(), "third");
}

#[test]
fn an_install_stopped_half_way_is_recorded_and_offered_again() {
    let card = temp("half");
    let src = temp("half-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/a.txt", b"a", false), ("Game-1.0/App/Game/b/c.txt", b"c", false)]);
    let mut p = port("game", &z, "1");
    p.categories = vec!["app".into()];
    // "App/Game/b" is a file on the card: c.txt cannot be put in it.
    fs::create_dir_all(card.join("App/Game")).unwrap();
    fs::write(card.join("App/Game/b"), "in the way").unwrap();
    assert!(s.install(&p, &AtomicBool::new(false), &mut |_| {}).is_err());
    let rec = s.card.installed("game").expect("a record of what was put on the card");
    assert!(rec.files.contains(&"App/Game/a.txt".to_string()));
    assert_eq!(s.state(&p), State::Update(portstore::install::INCOMPLETE.into()));
    assert!(!card.join("App/PortStore/cache/stage-game").exists());
}

#[test]
fn a_file_kept_and_now_skipped_is_never_deleted() {
    let card = temp("keep-skip");
    let s = store(&card);
    fs::create_dir_all(card.join("App/Game")).unwrap();
    fs::write(card.join("App/Game/settings.cfg"), "mine").unwrap();
    fs::create_dir_all(card.join("App/PortStore/installed")).unwrap();
    fs::write(
        card.join("App/PortStore/installed/game.json"),
        r#"{"id":"game","version":"1","files":["App/Game/settings.cfg"]}"#,
    )
    .unwrap();
    let p = Port {
        id: "game".into(),
        version: "1".into(),
        categories: vec!["app".into()],
        skip: vec!["App/Game/settings.cfg".into()],
        keep: vec!["App/Game/settings.cfg".into()],
        ..Port::default()
    };
    s.repair(&Index { format: 1, ports: vec![p], ..Index::default() });
    assert_eq!(fs::read_to_string(card.join("App/Game/settings.cfg")).unwrap(), "mine");
}

#[test]
fn a_tar_long_name_before_a_fifo_is_the_fifos() {
    let dir = temp("tar-fifo");
    let t = dir.join("a.tar.gz");
    let long = format!("App/X/{}", "f".repeat(120));
    tar_gz(&t, &[(&long, b"", b'6'), ("App/X/launch.sh", b"x", b'0')]);
    let names: Vec<String> = archive::members(&t, "tar.gz").unwrap().into_iter().map(|m| m.name).collect();
    assert_eq!(names, ["App/X/launch.sh"]);
}

#[test]
fn a_tar_cut_short_is_refused() {
    let dir = temp("tar-short");
    let mut t = tar_header("App/X/big.bin", 2000, b'0').to_vec();
    t.extend([7u8; 1024]);
    let path = dir.join("a.tar");
    fs::write(&path, t).unwrap();
    let err = archive::extract(&path, "tar", |_| true, |_, r| r.read_to_end(&mut Vec::new()).map(|_| ())).unwrap_err();
    assert!(err.to_string().contains("cut short"), "{err}");
}

#[test]
fn a_member_left_out_is_never_read() {
    let card = temp("left-out");
    let src = temp("left-out-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false), ("Game-1.0/Tools/tool.exe", b"MZ", false)]);
    patch_method(&z, "Game-1.0/Tools/tool.exe", 12);
    let mut p = port("game", &z, "1");
    p.categories = vec!["app".into()];
    p.file.sha256 = sha256(&z);
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(card.join("App/Game/launch.sh").exists());
}

#[test]
fn zip_names_without_the_utf8_flag_are_read_as_python_reads_them() {
    assert_eq!(archive::zip_name("Café/x".as_bytes(), false), "Café/x", "valid UTF-8 as such");
    assert_eq!(archive::zip_name(&[b'C', b'a', b'f', 0x82, b'/'], false), "Café/", "else code page 437");
    assert_eq!(archive::zip_name(&[0x82], true), "\u{fffd}");
}

#[test]
fn two_games_of_one_name_get_a_launcher_each() {
    let card = temp("same-name");
    let src = temp("same-name-src");
    let s = store(&card);
    let mk = |id: &str| {
        let z = src.join(format!("{id}.zip"));
        zip(&z, &[(&format!("Game-1.0/App/{id}/launch.sh"), b"x", false)]);
        let mut p = port(id, &z, "1");
        p.name = "Same Game".into();
        p.shortcut = Some(Shortcut { launch: format!("App/{id}/launch.sh"), ..Default::default() });
        p
    };
    let (a, b) = (mk("aaa"), mk("bbb"));
    s.install(&a, &AtomicBool::new(false), &mut |_| {}).unwrap();
    s.install(&b, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(fs::read_to_string(card.join("Roms/MiyooPorts/Same Game.sh")).unwrap().contains("App/aaa"));
    assert!(fs::read_to_string(card.join("Roms/MiyooPorts/Same Game (bbb).sh")).unwrap().contains("App/bbb"));
    // Starting again changes nothing (no ping-pong between the two).
    let before = fs::read_to_string(card.join("App/PortStore/installed/aaa.json")).unwrap();
    s.repair(&Index { format: 1, ports: vec![a.clone(), b.clone()], ..Index::default() });
    s.repair(&Index { format: 1, ports: vec![a, b], ..Index::default() });
    assert_eq!(fs::read_to_string(card.join("App/PortStore/installed/aaa.json")).unwrap(), before);
}

#[test]
fn a_launchers_picture_stays_until_a_new_one_is_at_hand() {
    let card = temp("picture-stays");
    let src = temp("picture-stays-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut p = port("game", &z, "1");
    p.shortcut = Some(Shortcut { launch: "App/Game/launch.sh".into(), ..Default::default() });
    p.cover = "img/game-cover-1.png".into();
    fs::create_dir_all(card.join("App/PortStore/img")).unwrap();
    fs::write(card.join("App/PortStore/img/game-cover-1.png"), "PNG1").unwrap();
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    // A newer catalog's picture, not downloaded yet.
    p.cover = "img/game-cover-2.png".into();
    s.repair(&Index { format: 1, ports: vec![p], ..Index::default() });
    assert_eq!(fs::read(card.join("Roms/MiyooPorts/Imgs/Game.png")).unwrap(), b"PNG1");
}

#[test]
fn removing_a_port_gone_from_the_catalog_keeps_its_kept_files() {
    let card = temp("keep-record");
    let src = temp("keep-record-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false), ("Game-1.0/App/Game/settings.cfg", b"d", false)]);
    let mut p = port("game", &z, "1");
    p.categories = vec!["app".into()];
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    s.remove("game", None).unwrap();
    assert!(card.join("App/Game/settings.cfg").exists());
    assert!(!card.join("App/Game/launch.sh").exists());
}

#[test]
fn versions_compare_as_people_mean_them() {
    use portstore::newer;
    assert!(newer("v1.2.0", "", "v1.2.0-rc2", ""), "the release after its candidate");
    assert!(!newer("v1.2.0-rc2", "", "v1.2.0", ""));
    assert!(!newer("v1.2.0", "", "v1.2", ""), "1.2 is 1.2.0");
    assert!(newer("v2.0", "2026-10-01", "2024.05.01", "2024-05-01"), "a year against a major: by date");
    assert!(newer("v1.3-x64", "", "v1.2-x64", ""));
    assert!(!newer("v1.2-x64", "", "v1.2-x64", ""));
    assert!(newer("v1.10", "", "v1.9", ""));
    assert!(newer("onion-v3", "", "onion-v2", ""));
}

#[test]
fn the_same_tag_uploaded_again_is_an_update() {
    let card = temp("same-tag");
    let src = temp("same-tag-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut p = port("game", &z, "onion-v2");
    p.categories = vec!["app".into()];
    p.date = "2026-10-01".into();
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(s.state(&p), State::Installed("onion-v2".into()));
    p.file.sha256 = "0".repeat(64);
    p.date = "2026-10-05".into();
    assert_eq!(s.state(&p), State::Update("onion-v2".into()));
}

#[test]
fn a_leftover_part_of_another_release_is_never_taken() {
    let card = temp("part");
    let src = temp("part-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut p = port("game", &z, "1");
    p.categories = vec!["app".into()];
    // A leftover of the same name, from before (not by its checksum): not looked at.
    fs::create_dir_all(card.join("App/PortStore/cache")).unwrap();
    fs::write(card.join("App/PortStore/cache/Game-1.0.zip"), "old release").unwrap();
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(card.join("App/Game/launch.sh").exists());
}

#[test]
fn a_wrong_file_in_downloads_is_set_aside_and_asked_for_again() {
    let card = temp("wrong-download");
    let src = temp("wrong-download-src");
    let mut s = store(&card);
    s.fetcher.curl = script(&src, "curl", "exit 6");
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut p = port("game", &z, "1");
    p.file.url = "https://example.org/Game-1.0.zip".into();
    fs::create_dir_all(card.join("App/PortStore/downloads")).unwrap();
    fs::write(card.join("App/PortStore/downloads/Game-1.0.zip"), "an older release").unwrap();
    assert!(s.install(&p, &AtomicBool::new(false), &mut |_| {}).is_err());
    assert!(card.join("App/PortStore/downloads/Game-1.0.zip.wrong").exists());
    let list = fs::read_to_string(card.join("App/PortStore/downloads/to-download.txt")).unwrap();
    assert!(list.contains("File: Game-1.0.zip"), "{list}");
}

#[test]
fn files_from_import_never_go_into_a_shared_folder_nor_over_a_rom() {
    let card = temp("import-guard");
    let home = card.join("App/PortStore");
    let s = store(&card);
    let put = |f: &str, data: &[u8]| {
        let p = home.join("import").join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    };
    put("Downloads/Fallout/master.dat", b"m");
    put("Downloads/holiday.jpg", b"j");
    put("TR1/DATA/level1.phd", b"l");
    let mut rom = vec![0x80, 0x37, 0x12, 0x40];
    rom.resize(0x40, 9);
    put("Downloads/mk64.z64", &rom);
    fs::create_dir_all(card.join("Roms/N64")).unwrap();
    fs::write(card.join("Roms/N64/mk64.z64"), "the user's other version").unwrap();
    let need = |check: &str| Need { what: "files".into(), check: vec![check.into()], ..Need::default() };
    let p = Port {
        needs: vec![
            need("Roms/PORTS/Games/Fallout/master.dat"),
            need("Roms/PORTS/Games/OpenLara/Tomb Raider 1/DATA"),
            Need {
                what: "ROM".into(),
                sha1: hex(&<sha1::Sha1 as sha1::Digest>::digest(&rom)),
                search: vec!["Roms/N64".into()],
                ..Need::default()
            },
        ],
        ..Port::default()
    };
    let mut roms = Roms::open(&home);
    let have = needs::check(&card, &p, &mut roms, &mut |_| {});
    let found = needs::importable(&card, &home, &p, &have, &mut roms);
    let to: Vec<Option<String>> = found.iter().map(|f| f.as_ref().map(|f| f.to.clone())).collect();
    assert_eq!(
        to,
        [
            Some("Roms/PORTS/Games/Fallout".into()),
            // A folder check: the folder holding DATA goes in as "Tomb Raider 1".
            Some("Roms/PORTS/Games/OpenLara/Tomb Raider 1".into()),
            Some("Roms/N64/mk64 (2).z64".into()),
        ]
    );
    for f in found.iter().flatten() {
        needs::place(&card, f).unwrap();
    }
    assert!(home.join("import/Downloads/holiday.jpg").exists(), "the rest stays in import");
    assert_eq!(fs::read_to_string(card.join("Roms/N64/mk64.z64")).unwrap(), "the user's other version");
    assert!(card.join("Roms/PORTS/Games/OpenLara/Tomb Raider 1/DATA/level1.phd").exists());
    let _ = s;
}

#[test]
fn the_updates_line_in_apps_is_written_whole() {
    let card = temp("note-atomic");
    let s = store(&card);
    let config = card.join("App/PortStore/config.json");
    fs::write(&config, r#"{"label": "Port Store", "description": "Install and update ports"}"#).unwrap();
    s.note_updates(2);
    assert!(!card.join("App/PortStore/config.json.part").exists());
    assert!(fs::read_to_string(&config).unwrap().contains("2 updates"));
}

#[test]
fn another_catalogs_port_takes_any_other_version_as_an_update() {
    let card = temp("source-version");
    let src = temp("source-version-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false)]);
    let mut p = port("game", &z, "dev-g67fc8b8");
    p.source = "slop.kitchen".into();
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(s.state(&p), State::Installed("dev-g67fc8b8".into()));
    // A hash that reads as a smaller number is still the newer build.
    p.version = "dev-g05fca8b".into();
    assert_eq!(s.state(&p), State::Update("dev-g67fc8b8".into()));
}

#[test]
fn development_builds_come_from_their_own_list() {
    let entry = |id: &str| {
        format!(
            r#"{{"id": "{id}", "name": "{id}", "version": "1", "take": ["App/"],
               "file": {{"url": "https://x/a.zip", "name": "a.zip", "size": 1, "sha256": "", "kind": "zip"}},
               "install": {{"files": 1, "bytes": 1}}}}"#
        )
    };
    let text = format!(r#"{{"format": 1, "ports": [{}], "dev": [{}]}}"#, entry("a"), entry("a-dev"));
    let index = Index::parse(&text).unwrap();
    let dev: Vec<(&str, bool)> = index.ports.iter().map(|p| (p.id.as_str(), p.dev)).collect();
    assert_eq!(dev, [("a", false), ("a-dev", true)]);
}

#[test]
fn keep_rules_with_patterns_cover_files_of_one_folder() {
    use portstore::index::keeps;
    let dir = "Roms/PORTS/Games/LastLight/";
    assert!(keeps("Roms/PORTS/Games/LastLight/meta.*", &format!("{dir}meta.json")));
    assert!(keeps("Roms/PORTS/Games/LastLight/run.?", &format!("{dir}run.1")));
    assert!(!keeps("Roms/PORTS/Games/LastLight/meta.*", &format!("{dir}meta/x.json")), "* stops at /");
    assert!(!keeps("Roms/PORTS/Games/LastLight/meta.*", &format!("{dir}other.json")));
    assert!(keeps("Roms/PORTS/Games/LastLight/", &format!("{dir}a/b")));
    assert!(keeps("App/Game/settings.cfg", "App/Game/settings.cfg"));
    assert!(!keeps("App/Game/settings.cfg", "App/Game/settings.cfg2"));
}

#[test]
fn an_update_keeps_files_a_pattern_keeps() {
    let card = temp("keep-pattern");
    let src = temp("keep-pattern-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"1", false), ("Game-1.0/App/Game/meta.json", b"shipped", false)]);
    let mut p = port("game", &z, "1.0");
    p.keep = vec!["App/Game/meta.*".into()];
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    fs::write(card.join("App/Game/meta.json"), "my progress").unwrap();
    let z2 = src.join("Game-2.0.zip");
    zip(&z2, &[("Game-1.0/App/Game/launch.sh", b"2", false), ("Game-1.0/App/Game/meta.json", b"new", false)]);
    let p2 = Port { file: port("game", &z2, "2.0").file, version: "2.0".into(), ..p.clone() };
    s.install(&p2, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(fs::read_to_string(card.join("App/Game/meta.json")).unwrap(), "my progress");
    s.remove("game", Some(&p2)).unwrap();
    assert!(card.join("App/Game/meta.json").exists(), "kept on removal");
}

#[test]
fn an_app_released_as_a_shortcut_for_ports_starts_from_miyooports() {
    let card = temp("app-shortcut");
    let src = temp("app-shortcut-src");
    let s = store(&card);
    let z = src.join("lab.zip");
    zip(
        &z,
        &[
            ("Roms/PORTS/Games/Lab/launch.sh", b"./lab", false),
            ("Roms/PORTS/Shortcuts/Tools/Sound Lab.port", b"#!/bin/sh\ncd /mnt/SDCARD/Roms/PORTS/Games/Lab && ./launch.sh\n", false),
        ],
    );
    let mut p = port("sound-lab", &z, "1");
    p.name = "Sound Lab".into();
    p.strip = String::new();
    p.categories = vec!["app".into()];
    let rec = s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(rec.shortcuts, "{:?}", rec.files);
    assert!(card.join("Roms/MiyooPorts/Sound Lab.sh").exists());
}

fn blob_id(bytes: &[u8]) -> String {
    use sha1::Digest;
    let mut h = sha1::Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    hex(&h.finalize())
}

fn brew(slug: &str, title: &str, system: &str, rom: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "slug": slug, "title": title, "system": system, "type": "game",
        "file": {
            "url": format!("https://raw.githubusercontent.com/gbdev/database/abc/entries/{slug}/{slug}.gb"),
            "name": format!("{slug}.gb"), "size": rom.len(), "sha1": blob_id(rom)
        },
        "shot": format!("https://raw.githubusercontent.com/gbdev/database/abc/entries/{slug}/shot.PNG")
    })
}

#[test]
fn the_homebrew_list_keeps_only_entries_from_github_raw() {
    let good = brew("a-game", "A Game", "GB", b"rom");
    let mut elsewhere = brew("b-game", "B", "GB", b"rom");
    elsewhere["file"]["url"] = "https://example.com/b.gb".into();
    let mut unknown = brew("c-game", "C", "N64", b"rom");
    unknown["system"] = "N64".into();
    let mut climbing = brew("../d", "D", "GB", b"rom");
    climbing["slug"] = "../d".into();
    let text = serde_json::json!({"format": 1, "entries": [good, elsewhere, unknown, climbing, {"slug": 1}]}).to_string();
    let list = portstore::homebrew::Homebrew::parse(&text).unwrap();
    let slugs: Vec<&str> = list.entries.iter().map(|e| e.slug.as_str()).collect();
    assert_eq!(slugs, ["a-game"]);
    assert_eq!(list.entries[0].folder(), "GB");
}

#[test]
fn homebrew_goes_into_its_systems_folder_and_comes_off_again() {
    let card = temp("homebrew");
    let s = store(&card);
    let rom = b"GB ROM BYTES".to_vec();
    let list = serde_json::json!({"format": 1, "generated": "2026-10-21", "entries": [
        brew("tetris-clone", "Blocks: The Game", "GB", &rom),
        brew("other", "Blocks: The Game", "GB", b"another"),
    ]});
    fs::write(card.join("App/PortStore/homebrew.json"), list.to_string()).unwrap();
    let list = s.load_homebrew();
    assert_eq!(list.entries.len(), 2);
    // Without Wi-Fi: the ROM brought into downloads/ is taken when it is the list's.
    fs::create_dir_all(card.join("App/PortStore/downloads")).unwrap();
    fs::write(card.join("App/PortStore/downloads/tetris-clone.gb"), &rom).unwrap();
    fs::write(card.join("App/PortStore/downloads/other.gb"), b"another").unwrap();
    let e = list.entry("tetris-clone").unwrap();
    let rec = s.install_homebrew(e, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(rec.rom, "Roms/GB/Blocks The Game.gb");
    assert_eq!(fs::read(card.join(&rec.rom)).unwrap(), rom);
    assert!(matches!(s.homebrew_state(e), State::Installed(_)));
    // Another of the same title never replaces it.
    let other = s.install_homebrew(list.entry("other").unwrap(), &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(other.rom, "Roms/GB/Blocks The Game (2).gb");
    // A newer ROM in the list is an update, and keeps the name (its saves go by it).
    let mut newer = e.clone();
    newer.file.sha1 = blob_id(b"NEWER");
    assert!(matches!(s.homebrew_state(&newer), State::Update(_)));
    fs::write(card.join("App/PortStore/downloads/tetris-clone.gb"), b"NEWER").unwrap();
    newer.file.size = 5;
    let again = s.install_homebrew(&newer, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(again.rom, rec.rom);
    assert_eq!(fs::read(card.join(&rec.rom)).unwrap(), b"NEWER");
    // ROMs are told by their git blob id, as git hash-object gives it.
    let blob = portstore::homebrew::blob_sha1(&card.join(&rec.rom), &AtomicBool::new(false)).unwrap();
    assert_eq!(blob, blob_id(b"NEWER"));
    s.remove_homebrew("tetris-clone").unwrap();
    assert!(!card.join(&rec.rom).exists());
    assert!(card.join(&other.rom).exists(), "the other stays");
    assert_eq!(s.homebrew_state(e), State::Absent);
}

#[test]
fn new_ports_are_announced_once_and_not_at_the_first_look() {
    let card = temp("arrivals");
    let s = store(&card);
    let port = |id: &str| Port { id: id.into(), name: id.into(), ..Port::default() };
    let mut index = Index { ports: vec![port("a"), port("b")], ..Index::default() };
    let (new, seen) = s.arrivals(&index);
    assert!(new.is_empty(), "the first look announces nothing");
    assert_eq!(seen["a"], "");
    index.ports.push(port("c"));
    let (new, seen) = s.arrivals(&index);
    assert_eq!(new, ["c"]);
    assert_eq!(seen["c"], portstore::today());
    assert!(s.arrivals(&index).0.is_empty(), "once");
}

#[test]
fn a_ports_own_cleanup_runs_before_its_files_go() {
    let card = temp("uninstall-hook");
    let src = temp("uninstall-hook-src");
    let s = store(&card);
    let z = src.join("Game-1.0.zip");
    let hook = b"echo \"$MIXTAPE_UNINSTALL $PORTSTORE_UNINSTALL\" > \"$SDCARD/cleaned\"\n";
    zip(&z, &[("Game-1.0/App/Game/launch.sh", b"x", false), ("Game-1.0/App/Game/mixtape-uninstall.sh", hook, false)]);
    let p = port("game", &z, "1.0");
    s.install(&p, &AtomicBool::new(false), &mut |_| {}).unwrap();
    s.remove("game", Some(&p)).unwrap();
    assert_eq!(fs::read_to_string(card.join("cleaned")).unwrap(), "1 1\n");
    assert!(!card.join("App/Game/mixtape-uninstall.sh").exists());
}

#[test]
fn port_store_can_have_an_entry_of_its_own_in_miyooports() {
    use portstore::miyooports::{self_entry, set_self_entry};
    let card = temp("self-entry");
    set_self_entry(&card, true).unwrap();
    assert!(self_entry(&card));
    let sh = fs::read_to_string(card.join("Roms/MiyooPorts/Port Store.sh")).unwrap();
    assert!(sh.contains("exec sh ./launch.sh"));
    assert!(card.join("Emu/MiyooPorts/config.json").exists());
    set_self_entry(&card, false).unwrap();
    assert!(!card.join("Roms/MiyooPorts/Port Store.sh").exists());
    // A game of that name is never taken for it.
    fs::write(card.join("Roms/MiyooPorts/Port Store.sh"), "#!/bin/sh\n").unwrap();
    assert!(set_self_entry(&card, true).is_err());
    set_self_entry(&card, false).unwrap();
    assert!(card.join("Roms/MiyooPorts/Port Store.sh").exists());
}
