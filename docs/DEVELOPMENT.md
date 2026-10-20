# Port Store: how it works, and how to work on it

This is the map for whoever continues Port Store: what each part does, how the catalog is
built and kept up to date by GitHub Actions, how the device app is written, and how to build,
test and release it. The [README](../README.md) covers using it and the recipe fields.

## The three parts

```
recipes/*.json ──► index/build.py (GitHub Actions, daily) ──► GitHub Pages: index.json + img/
                                                                   │
                      authors' GitHub releases ◄── downloads ── Port Store on the Mini (app/)
```

1. **Recipes** (`recipes/`): one JSON file per port, written by hand. Which repository, which
   release file, what of it goes onto the SD card, what the user has to bring.
2. **The catalog** (`index/build.py`): turns the recipes into `index.json`, with each port's
   newest release file, its SHA-256 and checked layout, and small pictures. Built by GitHub
   Actions and published on GitHub Pages:
   `https://l0st0npl4net.github.io/PortStore-Catalog-MiyooMini/index.json`.
3. **Port Store** (`app/`, Rust): the program on the Mini. Reads the catalog, downloads release
   files straight from their authors, checks them against the catalog and installs them.

Port Store hosts no ports: only the catalog and its pictures are on GitHub Pages.

## Repository layout

| Path | What |
|---|---|
| `recipes/miyoo64/`, `recipes/ports/` | recipes; `recipes/portstore.json` is Port Store itself |
| `index/build.py` | the catalog builder (Python 3, standard library; Pillow for pictures, py7zr for 7z) |
| `index/prune_cache.py` | drops cached release files the catalog no longer names |
| `index/report.py` | keeps the "Recipes failing" issue up to date (CI only) |
| `.github/workflows/index.yml` | builds and publishes the catalog |
| `.github/workflows/release.yml` | starts `index.yml` when a Port Store release is published |
| `app/` | Port Store (Cargo project, Rust 1.96 from `app/rust-toolchain.toml`) |
| `app/dist/` | files packaged with Port Store: `launch.sh`, OnionOS app `config.json`, the MiyooPorts system |
| `assets/` | font (DejaVu Sans), Port Store and MiyooPorts icons |
| `tools/package.sh`, `tools/release.sh` | build Port Store for the Mini, as a card folder or a release zip |
| `tools/icon.py`, `tools/crate_licenses.py` | Port Store's icon; the list of Rust crates and licenses |
| `docs/releases/` | release notes, one file per version, pasted into GitHub releases |
| `site/` (not in git) | a locally built catalog; `tools/package.sh` bundles it |

## The catalog and GitHub Actions

### When it is built

`.github/workflows/index.yml` runs:

- every day at 03:17 UTC (`schedule`), so new releases of the ports reach the devices;
- on a push to `main` that changes `recipes/`, `index/` or the workflow itself;
- by hand: Actions → index → Run workflow (`workflow_dispatch`);
- when a Port Store release is published: `release.yml` starts it with
  `gh workflow run index.yml --ref main`. It is started that way, not by a `release` trigger in
  `index.yml`, because GitHub Pages deploys only from `main`: a run started by a release would
  run on the tag and be refused by the Pages environment.

### What a run does

1. Python 3.12 and `pip install pillow py7zr` (pinned versions).
2. Restores `.cache` (the release files downloaded in earlier runs) from the Actions cache, so
   unchanged releases are not downloaded again (and the authors' download counts are not
   inflated).
3. `index/build.py --out site/index.json --cache .cache --previous <published index.json>`.
4. `index/prune_cache.py`: only the files the new catalog names stay in the cache.
5. `index/report.py`: opens, updates or closes the issue "Recipes failing" (label
   `recipe-failure`). A recipe is reported when it failed in this build and the one before, so a
   host down for a minute does not open issues. The issue closes by itself once all build.
6. Uploads `site/` (index and pictures) and deploys it to GitHub Pages.

Repository settings it needs: **Settings → Pages → Source: GitHub Actions**. The workflow
permissions (`contents: read`, `issues: write`, `pages: write`, `id-token: write`;
`actions: write` in `release.yml`) are in the files; the only secret is the automatic
`GITHUB_TOKEN`.

### What `build.py` does, per recipe

1. **Release:** the newest release of `github` whose file matches `asset` (and tag matches
   `tag`; pre-releases only with `prerelease`). With `GITHUB_TOKEN` through the GitHub API
   (5000 requests an hour); without it from github.com's own pages (`releases.atom` and
   `expanded_assets`), as the API allows only 60 requests an hour without a token.
2. **File:** downloaded into the cache, SHA-256 computed, checked against GitHub's own
   digest when GitHub gives one (only files uploaded since mid-2025 have it). Its kind is told
   by its first bytes (zip, tar, tar.gz, 7z), not by its name.
3. **Layout** (`plan`): members past `strip` (by default a single wrapper folder) go where
   `into`, `map` or `take` say, minus `skip`. Only what goes onto the card is checked for unsafe
   paths and links. Nothing taken: the recipe fails. Zips made on Windows have their
   backslashes read as folders. `target` in `build.py` and `install::target` in the app must
   stay the same.
4. **Needs:** the recipe's, plus the checks OnionOS's `~Import ports` would make: each Ports
   shortcut's `GameDir` + `GameDataFile` that the release does not bring.
5. **Pictures:** the release's own (OnionOS `Roms/PORTS/Imgs`, else an app's `icon.png`), else
   the catalog's `image`, made 48×48 (`thumb`) and up to 224×168 (`cover`) PNGs in `site/img/`,
   named with a hash of their content so devices never keep a stale one.
6. **Install facts:** file count and size, `roots` (the port's own folders) and `marks` (one
   file in each: all on the card means the port is there, installed by hand).

A recipe that fails keeps its entry from the previous catalog (`--previous`), so a renamed
release file does not take a port away from the devices. Catalog entries of
[Producdevity/MiyooMini-Ports](https://github.com/Producdevity/MiyooMini-Ports) without a recipe
are listed under `manual` (the More tab).

### `index.json`

`format` (1; Port Store refuses others), `generated` (UTC time; the newest catalog at hand
wins), `ports`, `manual`, `failed`. A port:

| Field | |
|---|---|
| `id`, `name`, `group`, `categories`, `status`, `assets`, `porter`, `notes`, `about`, `url` | shown in the app; category `app` puts it under Apps, `group` under a heading |
| `thumb`, `cover` | pictures, relative to the catalog's address |
| `version`, `date` | the release's tag and date |
| `file` | `url`, `name`, `size`, `sha256`, `kind` (`zip`, `tar`, `tar.gz`, `7z`) |
| `strip`, `take`, `into`, `map`, `skip`, `keep`, `shortcut` | install rules, as in the recipe |
| `needs` | what the user brings: `what`, `hint`, `check`, `sha1`, `search`, `n64`, `title` |
| `install` | `files`, `bytes`, `roots`, `marks` |

New fields must be optional on the device (`#[serde(default)]`), so older Port Stores keep
reading new catalogs; a change older ones cannot read needs `format` 2.

### Adding a port

1. Look at the release file's layout (`unzip -l`, `tar tzf`).
2. Write `recipes/ports/<id>.json`: `catalog` (its name in MiyooMini-Ports) or `name`,
   `github`, `asset`. Add `into` / `map` when the layout is not `App/…` or `Roms/PORTS/…`;
   `needs` when the user brings files; `keep` for settings files the release ships.
3. Build it locally: `python3 index/build.py --only <id> --out /tmp/idx/index.json --cache .cache`
   (shows the files taken and those left out).
4. Try it on a card folder (see "Testing" below), then push: the catalog is rebuilt.

## Port Store (`app/`)

### Modules

| File | What |
|---|---|
| `main.rs` | command line: no arguments → the screens; `list`, `install`, `remove`, `needs`, `refresh`, `shots` for testing |
| `lib.rs` | `Store`: the card and the catalog's address. Loading the newest catalog at hand, `refresh`, `state` of a port, `install` (find or download the file, check it, install), `remove`, Ports Collection, the list for a computer (`to-download.txt`), version comparison (`newer`) |
| `index.rs` | the catalog's types (`Index`, `Port`, `Need`, `Shortcut`…) |
| `fetch.rs` | downloads with OnionOS's curl (`/mnt/SDCARD/.tmp_update/bin/curl`, certificates checked against the bundled `cacert.pem`), progress, cancel; `file://` copied directly; SHA-256 |
| `archive.rs` | zip (stored, deflate; CRC checked), tar, tar.gz readers of its own; 7z unpacked whole by OnionOS's 7-Zip into a folder, then read as one |
| `install.rs` | `Card`: placing a port's files, install records, updates, removal, `conform` (bringing installed ports in line with how Port Store installs now) |
| `miyooports.rs` | the MiyooPorts system: where Ports files are kept hidden, launchers, the system's files |
| `needs.rs` | what the user brings: files in place (as `~Import ports` finds them), N64 ROMs by SHA-1 in any byte order (cached in `cache/roms.json`) |
| `ui/mod.rs` | the screens: `App` (state, keys, jobs, drawing), tabs, lists, a port's page, dialogs |
| `ui/fb.rs`, `ui/input.rs` | the framebuffer (640×480, turned half a turn, 2 pages) and the buttons (evdev) |
| `ui/canvas.rs` | drawing: rectangles, text (fontdue, DejaVu Sans), pictures, PNG output |
| `ui/pictures.rs` | pictures: bundled, cached or downloaded on a thread of their own |

No SDL: Port Store draws into a buffer and copies it to the framebuffer, and reads the buttons
from `/dev/input/event*`. It only works at 640×480 (the Mini and Mini Plus); the Mini v4 and
Flip (752×560) need the layout to follow the screen size.

### Threads

The screen thread owns `App`: keys, drawing, and messages (`Msg`) from the others through one
channel. A job (download and install, a queue of them, removal, the catalog) runs on a thread of
its own and reports progress (`Step`) and its end (`Finished`); B cancels downloads through an
`AtomicBool`. Checks of a port's needs and the catalog's download on start run on threads too;
pictures have one thread that downloads them one by one. `Store` is shared (`Arc`); the catalog
belongs to the screen thread and is replaced whole when a newer one comes.

### On the card

```
App/PortStore/
  portstore, launch.sh, config.json   the program; OnionOS's Apps entry
  settings.json                       {"index": "<catalog address>"}
  index.json, img/                    the catalog and pictures it was released with
  cacert.pem, licenses/
  installed/<id>.json                 per port: version, date, every file put on the card
  downloads/                          release files and index.json brought on a computer;
                                      to-download.txt, what Port Store asks for
  cache/                              last catalog, pictures, ROM checksums, log, staging
```

`launch.sh` runs the program with OnionOS's libraries in `LD_LIBRARY_PATH` (for curl), logs to
`cache/portstore.log`, and runs `~Import ports` on exit when `cache/import` was left.

### Installing

1. The file: from `downloads/` when it is there and its SHA-256 matches, else downloaded into
   `cache/`; a wrong checksum stops everything.
2. Free space checked; every member that goes onto the card unpacked into
   `cache/stage-<id>/` (a broken file stops the job before the card is touched).
3. A game's `App/<Name>/config.json` (its entry in Apps) is read for how it starts and not
   installed.
4. Files moved into place; `keep` files that exist are left. The archive's empty folders made.
5. MiyooPorts launchers written; `Roms/PORTS/Games` made when Ports Collection is used.
6. An update removes the files the old version had and the new one has not; files another
   installed port has are never removed.
7. The record written; Ports Collection added when a shortcut needs it.

Removal deletes the recorded files (not `keep` ones, not other ports'), then the folders left
empty, except shared ones (`App`, `Roms/<System>`, Shortcut categories, MiyooPorts' folders).
Anything the user put in a port's folders stays.

### MiyooPorts

A system in Games of Port Store's own: `Emu/MiyooPorts/config.json` (`rompath`
`Roms/MiyooPorts`, `extlist` `sh`) and `launch.sh`, with the icon
`Icons/Default/miyooports.png` (icon packs may bring their own `miyooports.png`: OnionOS finds
icons by file name). They are written by `miyooports::ensure_system` from files built into the
program (`app/dist/miyooports/`, `assets/miyooports.png`).

Each game gets `Roms/MiyooPorts/<Name>.sh` and `Roms/MiyooPorts/Imgs/<Name>.png`:

- **release for Ports Collection:** its `Roms/PORTS/Shortcuts`, `Imgs`, `Manuals` go to
  `Roms/MiyooPorts/.ports/` (MainUI lists no name starting with a dot, so a game shows once);
  the launcher runs the shortcut through `Emu/PORTS/launch.sh`, as Ports would. The game stays
  in `Roms/PORTS/Games`, where those shortcuts look for it;
- **game released as an app:** the launcher starts its launcher in `App/<Name>` (from its
  `config.json`, kept in the record's `apps`), with OnionOS's audio shim preloaded as Apps have;
- **`shortcut` in the recipe:** that launcher, for releases whose own shortcut does not work.

Before the game, the launcher checks each `needs` entry with `check` paths and shows
`infoPanel` with what is missing and where it goes. After changes MainUI's list caches
(`Roms/MiyooPorts/MiyooPorts_cache6.db` and `_cache2.db`) are deleted so the list is read again.

Apps (category `app`) get no launcher and stay in Apps. On start, `Store::repair` runs `conform`
for every installed port: games of Port Store 0.3 move from `Roms/PORTS/Shortcuts` and Apps into
MiyooPorts, launchers are made or brought up to date (written only when they differ).

### Updates and Port Store's own updates

- On start Port Store downloads the catalog quietly (the footer says "offline" when it cannot);
  Y downloads it again. The newest catalog at hand is used: bundled, `cache/index.json` or
  `downloads/index.json`, by `generated`.
- A port has an update when the catalog's release is newer than the installed one (`newer`:
  the numbers in the versions when both have some, else the release dates, else just being
  another); a catalog behind the device offers nothing.
- Port Store compares itself by its own version (`CARGO_PKG_VERSION`), as it may have come
  onto the card unzipped, without an install record. When the catalog's Port Store is newer it
  is offered at once; once installed (over the running program, which keeps running from the
  replaced file), Port Store starts the new one with `exec`. `settings.json` is `keep`.

### Without Wi-Fi

When curl reports no connection (codes 6, 7, 28), Port Store writes
`downloads/to-download.txt`: the catalog's address, then each wanted file's name and address.
Files brought into `downloads/` are used when their checksum matches; `downloads/index.json`
is used when it is the newest catalog. Files listed and already brought drop off the list.

## Building, testing, releasing

### Machine

- Rust 1.96 with the `armv7-unknown-linux-gnueabihf` target (`app/rust-toolchain.toml`).
- The Miyoo Mini toolchain (GCC 8.3, glibc 2.28) as linker; the environment is set by
  `../slot-mini/tools/mini/env.sh` (linker, `CC`, `-C target-cpu=cortex-a7`).
- For the catalog: Python 3; Pillow and py7zr for pictures and 7z.
- `qemu-arm` to run the Mini build off the device.

### Testing

- `cd app && cargo test`: archives, installs, updates, removal, MiyooPorts launchers (run with
  `sh` against a card folder), offline list, version comparison, list ordering.
- A card in a folder: `PORTSTORE_CARD=/tmp/card PORTSTORE_HOME=/tmp/card/App/PortStore
  target/debug/portstore install <id>` (release files can be placed in `downloads/`;
  `PORTSTORE_CURL`, `PORTSTORE_7Z` replace curl and 7-Zip).
- Screens as PNGs: `portstore shots DIR [ID…]` (all tabs, a port's page, a dialog).
- The Mini build under qemu:
  `qemu-arm -cpu cortex-a7 -L <toolchain>/arm-linux-gnueabihf/libc App/PortStore/portstore list`.

### Releasing

1. Bump `version` in `app/Cargo.toml` and write `docs/releases/v<version>.md`.
2. `tools/release.sh https://l0st0npl4net.github.io/PortStore-Catalog-MiyooMini/index.json`
   → `out/PortStore-<version>.zip` (bundles `site/index.json` and `site/img/` if built).
3. Push, then publish a GitHub release tagged `v<version>` with that zip and the notes.
   `release.yml` rebuilds the catalog; devices are offered the update on their next start.

`tools/package.sh OUT URL` gives the same as a card folder (`OUT/App/PortStore`), with an
install record for Port Store.

## Things to know about OnionOS

- The SD card is FAT32: no links, no execute bits (scripts run with `sh`), names compared
  without case.
- `~Import ports` (`Emu/PORTS/import.sh`) shows a Ports shortcut only when its `GameDataFile`
  is up to two folders below `Roms/PORTS/Games/<GameDir>`, renaming it `.notfound` otherwise.
  `GameDir` may climb out (`../../../App/<Game>`); then `Roms/PORTS/Games` must exist.
- Ports Collection is a Package Manager package (`App/PackageManager/data/Emu/Ports
  Collection`): installing it is copying it to the card's root and running its `install.sh`.
- MainUI (Open MainUI in RedOnion) keeps a list cache per folder,
  `<folder>/<folder name>_cache6.db`; deleting it makes it read the folder again.
- keymon owns the power and volume buttons; Port Store reads the others.

## Open items

- Mini v4 and Mini Flip: a layout for 752×560.
- Recipes sent as pull requests: build only the changed recipe and comment the result.
- Help with game files: copy a PC game folder into one place, Port Store puts the files where
  the port wants them.
- The number of updates in Port Store's description in Apps.
- Releases made of several files (Super Haxagon), `.ipk` packages.
