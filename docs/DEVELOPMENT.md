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
| `index/report.py` | keeps the "Recipes failing" issue up to date (CI only) |
| `index/check_recipes.py` | checks and builds given recipes, reports in Markdown (pull requests) |
| `.github/workflows/index.yml` | builds and publishes the catalog |
| `.github/workflows/release.yml` | starts `index.yml` when a Port Store release is published |
| `.github/workflows/recipes.yml` | checks the recipes a pull request adds or changes, and comments the result |
| `app/` | Port Store (Cargo project, Rust 1.96 from `app/rust-toolchain.toml`) |
| `app/dist/` | files packaged with Port Store: `launch.sh`, OnionOS app `config.json`, the MiyooPorts system |
| `assets/` | font (DejaVu Sans), Port Store and MiyooPorts icons |
| `tools/package.sh`, `tools/release.sh` | build Port Store for the Mini, as a card folder or a release zip |
| `tools/fetch_catalog.py` | puts the published catalog and its pictures into a folder (bundled with a release) |
| `tools/icon.py`, `tools/crate_licenses.py` | Port Store's icon; the list of Rust crates and licenses |
| `docs/releases/` | release notes, one file per version, pasted into GitHub releases |
| `site/` (not in git) | a locally built catalog; `tools/package.sh` bundles it |

## The catalog and GitHub Actions

### When it is built

`.github/workflows/index.yml` runs:

- every day at 03:17 UTC (`schedule`), so new releases of the ports reach the devices;
- on a push to `main` that changes `recipes/`, `index/` or the workflow itself;
- by hand: Actions → index → Run workflow (`workflow_dispatch`);
- when a Port Store release is published or edited (a file attached later): `release.yml` starts it with
  `gh workflow run index.yml --ref main`. It is started that way, not by a `release` trigger in
  `index.yml`, because GitHub Pages deploys only from `main`: a run started by a release would
  run on the tag and be refused by the Pages environment.

### What a run does

1. Python 3.12 and `pip install pillow py7zr` (pinned versions).
2. Restores `.cache` (the release files downloaded in earlier runs) from the Actions cache, so
   unchanged releases are not downloaded again (and the authors' download counts are not
   inflated).
3. `index/build.py --out site/index.json --cache .cache --previous <published index.json>`.
   The previous index is tried three times; when it cannot be had (but exists), the build
   stops rather than publish a catalog without the ports failing now. Only the release files
   this build used stay in the cache.
4. `index/report.py`: keeps one issue "Recipes failing" (label `recipe-failure`) up to date:
   recipes that failed in this build and the one before (a host down for a minute opens
   nothing), and warnings (a newer release passed over because it has no file matching
   `asset`). The last such issue is reopened rather than a new one made; it is closed once
   nothing fails.
5. Uploads `site/` (index and pictures) and deploys it to GitHub Pages.

Repository settings it needs: **Settings → Pages → Source: GitHub Actions**. Permissions are
per job: the build, which reads untrusted archives, gets `contents: read` and `issues: write`;
the deploy gets `pages: write` and `id-token: write`; `release.yml` gets `actions: write`. The
only secret is the automatic `GITHUB_TOKEN`.

### What `build.py` does, per recipe

1. **Release:** the newest release of `github` whose file matches `asset` (and tag matches
   `tag`; pre-releases only with `prerelease`). With `GITHUB_TOKEN` through the GitHub API
   (1000 requests an hour for the repository's token in Actions); without it from github.com's own pages (`releases.atom` and
   `expanded_assets`), as the API allows only 60 requests an hour without a token.
2. **File:** downloaded into the cache (named by GitHub's asset id and upload time, so a
   file uploaded again under the same name is downloaded again; its length is checked, and
   files over 2 GB refused), SHA-256 computed, checked against GitHub's own digest when GitHub
   gives one (only files uploaded since mid-2025 have it; a mismatch is downloaded once more).
   Its kind is told by its first bytes (zip, tar, tar.gz, 7z), not by its name.
3. **Layout** (`plan`): members past `strip` (by default a single wrapper folder) go where
   `into`, `map` or `take` say, minus `skip`. Only what goes onto the card is checked for unsafe
   paths and links, and for what the device cannot read: zip members encrypted, compressed
   other than stored or deflate, or with a NUL in the name (left-out members may be anything).
   Nothing taken: the recipe fails. Zip names are read as the device reads them (UTF-8 when
   flagged or valid, else code page 437; Windows backslashes as folders); 7z links are told
   apart. `target`/`dos_name` in `build.py` and `install::target`/`archive::zip_name` in the
   app must stay the same.
4. **Needs:** the recipe's, plus the checks OnionOS's `~Import ports` would make: each Ports
   shortcut's `GameDir` + `GameDataFile` that the release does not bring.
5. **Pictures:** the release's own (OnionOS `Roms/PORTS/Imgs`, else an app's `icon.png`), else
   the catalog's `image`, made 48×48 (`thumb`) and up to 224×168 (`cover`) PNGs in `site/img/`,
   named with a hash of their content so devices never keep a stale one.
6. **Install facts:** file count and size, `roots` (the port's own folders) and `marks` (one
   file in each: all on the card means the port is there, installed by hand).
7. **Types:** the entry is checked against the device's types (`check_port`); metadata from
   MiyooMini-Ports of another type is dropped field by field (`clean_meta`), and its entries
   under `manual` likewise.

Whatever a recipe throws (a broken archive, a cut-off download, a wrong field type, an id
another recipe has too) fails that recipe only, never the build. Members read for shortcuts
and pictures are refused past 1 MB and 16 MB. A recipe that fails keeps its entry from the previous catalog (`--previous`), so a renamed
release file does not take a port away from the devices. Catalog entries of
[Producdevity/MiyooMini-Ports](https://github.com/Producdevity/MiyooMini-Ports) without a recipe
are listed under `manual` (the More tab).

### `index.json`

`format` (1; Port Store refuses others), `generated` (UTC time; the newest catalog at hand
wins), `ports`, `manual`, `failed`, `warnings`. The device reads each entry on its own: one it
cannot read, or one with a path leading off the card (absolute, `..`, backslash, an `id` that
is not a plain name), is left out and logged, the rest of the catalog stands. A port:

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

### Recipes in pull requests

`recipes.yml` runs `index/check_recipes.py` on the recipe files a pull request adds or
changes (renames included): fields (unknown ones, types, paths on the card, `id` unique across
all recipes, `github`, regular expressions, `needs`, `map`), the `catalog` name, then the same build as the catalog's (release, download, layout, needs,
pictures). The report goes to the run's summary and as a comment on the pull request
(edited on later pushes; pull requests from forks may not comment, the summary is there). The
check fails when a recipe does.

### Adding a port

1. Look at the release file's layout (`unzip -l`, `tar tzf`).
2. Write `recipes/ports/<id>.json`: `catalog` (its name in MiyooMini-Ports) or `name`,
   `github`, `asset`. Add `into` / `map` when the layout is not `App/…` or `Roms/PORTS/…`;
   `needs` when the user brings files; `keep` for settings files the release ships.
3. Check it locally: `python3 index/check_recipes.py recipes/ports/<id>.json`, or build it:
   `python3 index/build.py --only <id> --out /tmp/idx/index.json --cache .cache` (`--only`
   needs `--out`, so `site/` never holds a partial catalog).
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
| `needs.rs` | what the user brings: files in place (as `~Import ports` finds them), N64 ROMs by SHA-1 in any byte order (cached in `cache/roms.json`); files brought into `import/` and where they go |
| `ui/mod.rs` | the screens: `App` (state, keys, jobs, drawing), tabs, lists, a port's page, dialogs |
| `ui/fb.rs`, `ui/input.rs` | the framebuffer (640×480, turned half a turn, 2 pages) and the buttons (evdev) |
| `ui/canvas.rs` | drawing: rectangles, text (fontdue, DejaVu Sans), pictures, PNG output |
| `ui/pictures.rs` | pictures: bundled, cached or downloaded on a thread of their own |

No SDL: Port Store draws into a buffer and copies it to the framebuffer, and reads the buttons
from `/dev/input/event*`. The layout follows the framebuffer's size (640×480 at least): the
`full_resolution` file next to the program makes OnionOS give apps the panel's own 752×560 on
the Mini v4 and Flip, and the lists get wider and longer. `PORTSTORE_SHOT_SIZE=752x560` makes
`shots` draw that size.

### Threads

The screen thread owns `App`: keys, drawing, and messages (`Msg`) from the others through one
channel. A job (download and install, a queue of them, removal, the catalog) runs on a thread of
its own and reports progress (`Step`) and its end (`Finished`); B cancels downloads through an
`AtomicBool` (only jobs that download offer it). The job runs under `catch_unwind` (the release
build unwinds), so a panic still ends it with a message and the screen is never left busy.
Checks of a port's needs and the catalog's download on start run on threads too; each check
carries a number and only a port's newest one is taken. Pictures have one thread that downloads
them one by one. `Store` is shared (`Arc`); the catalog belongs to the screen thread and is
replaced whole when a newer one comes: one downloaded on start while a job runs is held until
the job is done (its `repair` must not change a port the job is installing). Each catalog
download writes a file of its own, so the one on start and a Y never clash.

### On the card

```
App/PortStore/
  portstore, launch.sh, config.json   the program; OnionOS's Apps entry
  settings.json                       {"index": "<catalog address>"}
  index.json, img/                    the catalog and pictures it was released with
  cacert.pem, licenses/
  installed/<id>.json                 per port: version, date, SHA-256, every file put on
                                      the card, its keep rules, how it starts
  import/                             game files brought on a computer, in any layout
  downloads/                          release files and index.json brought on a computer;
                                      to-download.txt, what Port Store asks for
  cache/                              last catalog, pictures, ROM checksums, log, staging,
                                      downloads by checksum (<sha>-<name>)
```

`launch.sh` runs the program with OnionOS's libraries in `LD_LIBRARY_PATH` (for curl), logs to
`cache/portstore.log`, and runs `~Import ports` on exit when `cache/import` was left.

### Installing

1. The file: from `downloads/` when it is there and its SHA-256 matches (one that does not
   is renamed `.wrong` and asked for again), else downloaded into `cache/<sha>-<name>` (a part
   as long as the file is dropped; a wrong checksum is downloaded once more from nothing, then
   stops everything). The download is deleted after the install, or after an archive error;
   after another error (no space left) it stays for the next try.
2. Every member that goes onto the card is checked: a safe name, no link, a target path on
   the card. Members landing on one path in any case (FAT32 tells no case apart) are one file
   there: the last one's content, the first one's spelling. Free space checked; those members
   only (others are not even read) unpacked into `cache/stage-<id>/`: a broken file stops the
   job before the card is touched.
3. A game's `App/<Name>/config.json` (its entry in Apps) is read for how it starts and not
   installed.
4. Files moved into place; `keep` files that exist are left. The archive's empty folders made.
   A move that fails stops the job, and the record then holds everything on the card (the old
   version's files and the new ones moved), with the old version, or `(incomplete)`: the port
   is offered again, and Remove still finds all.
5. MiyooPorts launchers written; `Roms/PORTS/Games` made when Ports Collection is used.
6. An update removes the files the old version had and the new one has not, compared in any
   case; files another installed port has (in any case) are never removed.
7. The record written; Ports Collection added when a shortcut needs it.

Removal deletes the recorded files (not `keep` ones, by the catalog's rules or the record's,
not other ports', not a path off the card), then the folders left empty, except shared ones
(`App`, `Roms/<System>`, Shortcut categories, MiyooPorts' folders). Anything the user put in a
port's folders stays. Port Store never removes itself.

### MiyooPorts

A system in Games of Port Store's own: `Emu/MiyooPorts/config.json` (`rompath`
`Roms/MiyooPorts`, `extlist` `sh`) and `launch.sh`, with the icon
`Icons/Default/miyooports.png` (icon packs may bring their own `miyooports.png`: OnionOS finds
icons by file name). They are written by `miyooports::ensure_system` from files built into the
program (`app/dist/miyooports/`, `assets/miyooports.png`).

Each game gets `Roms/MiyooPorts/<Name>.sh` and `Roms/MiyooPorts/Imgs/<Name>.png` (a name
another installed port's launcher has gets ` (<id>)` after it; a picture stays as it is while
no new one is at hand):

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
- A port has an update when the catalog's release is newer than the installed one (`newer`):
  the first run of numbers in each version ("v1.2.0-rc2" → 1.2.0, trailing zeros dropped, so
  1.2 = 1.2.0), a pre-release (`-rc2`, `beta`) before the bare numbers; when one is date-like
  (2024.05.01) and the other not, or neither has numbers, the release dates; else just being
  another. The same tag with another SHA-256 and a later date (a file uploaded again) is an
  update too. A catalog behind the device offers nothing.
- Port Store compares itself by its own version (`CARGO_PKG_VERSION`, numbers only: "0.5.0"
  and "v0.5.0" are one), as it may have come onto the card unzipped, without an install
  record. When the catalog's Port Store is newer it is offered once the list is on screen and
  nothing runs; once installed (over the running program, which keeps running from the
  replaced file), Port Store starts the new one with `exec`. `settings.json` is `keep`.

### Game files from `import/`

For each missing need, `needs::importable` looks in `import/` (6 levels deep): for a `check`
path `D/f`, the shallowest file or folder named `f` (any case), found as `X/f`; the contents of
`X` go into `D`. When `X` and `D` end in the same folder names (any case), both go that many
levels up, as long as `X` stays a folder of its own in `import/` and `D` never becomes a folder
the card shares (`Roms/PORTS/Games`, `Roms/<System>`, `App`): so "RollerCoaster Tycoon 2/Data/
G1.DAT" for `…/OpenRCT2mini/rct2/Data/g1.dat` moves the whole RCT2 folder as `rct2`, and
"Downloads/Fallout/master.dat" moves only the Fallout folder. A placement inside another's
folder is dropped (it moves with it). A ROM need takes a ROM of the right SHA-1, moved into the
first `search` folder under a free name (`mk64 (2).z64` beside another version). `needs::place`
moves file by file (the user's copy replaces files of the same name, removed only once the move
can be made), refuses a target off the card, and removes what is left of the folder. Recipes should name a file in the game's own layout in `check` (Half-Life:
`valve/liblist.gam`), so the folder lands where the port wants it.

### Updates in Apps

After each look at the catalog, `Store::note_updates` writes the number of updates into Port
Store's `description` in its `config.json` ("Install and update ports · 3 updates"), which
MainUI shows under its name in Apps; the file is rewritten only when the line changes (through
a temporary file, so a flat battery never leaves it half written), keeping keys OnionOS's icon
packs set.

### Without Wi-Fi

curl gets 20 s to connect and gives up on a transfer stalled for 30 s. When it reports no
connection (codes 6, 7, 28), Port Store writes
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
   → `out/PortStore-<version>.zip`, with the published catalog and its pictures bundled
   (`tools/fetch_catalog.py`; `PORTSTORE_LOCAL_CATALOG=1` takes `site/` instead).
3. Push, then publish a GitHub release tagged `v<version>` **with that zip attached** and the
   notes. `release.yml` rebuilds the catalog (also when the release is edited, e.g. the file
   attached later); devices are offered the update on their next start. A release published
   without its file shows as a warning in the "Recipes failing" issue.

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

- Mini v4 and Mini Flip: checked on screenshots only; needs a try on the devices.
- curl processes still running (the catalog's download on start, a picture) are not stopped
  when Port Store exits; they end by themselves.
- The number of updates shows once Port Store has run; a check at boot would show it sooner.
- Releases made of several files (Super Haxagon), `.ipk` packages.
