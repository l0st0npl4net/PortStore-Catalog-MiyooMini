# Port Store for Miyoo Mini

Install, update and remove ports right on a Miyoo Mini or Mini Plus running
[Onion](https://github.com/OnionUI/Onion): no computer, no copying folders by hand.

![Games](docs/screenshots/games.png) ![A port's page](docs/screenshots/port.png)

- **60 ports and apps**, including all 12 native N64 games of
  [Miyoo64](https://github.com/mr-finster/miyoo64) (Mario Kart 64, Super Mario 64, Ocarina
  of Time, GoldenEye 007…), Undertale, Fallout 1 and 2, Half-Life, Tomb Raider and apps
  like Moonlight, Syncthing or Better WiFi.
- **Games** and **Apps** tabs, genres (Select), pictures, and an **Updates** tab when a port's
  author publishes a new version.
- Games go to Onion's **Games → Ports**, next to your consoles.
- For games that need your own files (a ROM, the game's data) Port Store says what to put
  where and checks it: N64 ROMs are recognised by checksum, in any byte order.
- Everything is downloaded from the authors' own GitHub releases and checked against the
  catalog's SHA-256. Port Store hosts no games.

## Install

1. Download **`PortStore-<version>.zip`** from
   [Releases](https://github.com/l0st0npl4net/PortStore-Catalog-MiyooMini/releases).
2. Unzip it to the **root of the SD card**. It contains `App/PortStore`: if asked, merge
   with the `App` folder that is already there.
3. On the Mini: **Apps → Port Store**.

Needs Wi-Fi (Mini Plus) and the right date and time (Clock app) for secure downloads.
Port Store draws for the 640×480 screen: the Mini v4 and Mini Flip are not supported yet.

**Mini without Wi-Fi:** the catalog comes with Port Store. On a computer, download the file
named on a port's page (`File: …`) from the port's release and put it in
`App/PortStore/downloads` on the card; Port Store installs from there.

**Games → Ports** needs Onion's *Ports Collection*. If it is not on the card, Port Store
offers to add it (as Package Manager would).

## Use

| Button | |
|---|---|
| D-pad | move; Left / Right: a page |
| L1 / R1 | tabs: Games, Apps, Installed, Updates, More |
| Select | Games: next genre |
| A | open a port; on its page: install, update or reinstall |
| X | remove (on the page of an installed port) |
| Y | list: download the newest catalog; page: check your files again |
| B | back; on the list: quit |

- **More** lists ports Port Store cannot install yet, with a link to their page.
- Removing a port takes away the files Port Store put on the card. Your saves, settings and
  game files stay.
- Port Store updates itself: it shows up under Updates when a new release is out. Start it
  again afterwards.

Troubleshooting: Port Store writes `App/PortStore/cache/portstore.log`; Onion's import of
ports writes `Roms/PORTS/import.log`.

## Thanks

- [mr-finster/miyoo64](https://github.com/mr-finster/miyoo64): the N64 decompilation ports
  for the Mini.
- [Producdevity/MiyooMini-Ports](https://github.com/Producdevity/MiyooMini-Ports): the
  catalog of Mini ports; Port Store takes its categories, status, notes and pictures, and
  lists its other entries under More.
- Every port's author, whose releases Port Store installs, and
  [Onion](https://github.com/OnionUI/Onion).

---

## How it works

- `recipes/` — one JSON file per port: which release file to take and what of it goes onto
  the SD card.
- `index/build.py` — builds `index.json` from the recipes: newest matching release, the
  file's SHA-256, its layout checked, the pictures made small. GitHub Actions runs it every
  day and when recipes change, and publishes the result on GitHub Pages.
- `app/` — Port Store itself (Rust, draws to the framebuffer, no SDL).

Only the ports' pictures are published with the index (`img/`: 48×48 for the list, up to
224×168 for a port's page), taken from the release (Onion's `Roms/PORTS/Imgs`, or an app's
`icon.png`) or else from the catalog's `image`. The build needs Pillow for them
(`pip install pillow`); without it the index is built without pictures.

### Recipes

```json
{
  "id": "n64-mario-kart-64",
  "name": "Mario Kart 64",
  "group": "Miyoo64 (N64)",
  "github": "mr-finster/miyoo64",
  "asset": "^MarioKart64-miyoo\\.zip$",
  "take": ["App/MarioKart64/", "Roms/PORTS/"],
  "skip": ["App/MarioKart64/config.json"],
  "needs": [{ "what": "ROM: Mario Kart 64 (USA)", "sha1": "579c…", "search": ["Roms/N64"], "n64": true }]
}
```

| Field | |
|---|---|
| `id` | unique, also the name of the install record on the card |
| `catalog` | the entry's `name` in MiyooMini-Ports `ports.json`: its metadata is used |
| `name`, `group`, `categories`, `status`, `assets`, `porter`, `about`, `url`, `image` | override or replace catalog metadata. Category `app` puts a port under Apps, the rest under Games; ports of a `group` are listed together under its heading |
| `github`, `asset`, `tag`, `prerelease` | release search: repository, file name regex, tag regex, allow pre-releases |
| `strip` | prefix taken off every member (default: a single wrapper folder that is not a card folder) |
| `take` | path prefixes (from the card's root) that are installed; default `App/`, `Roms/PORTS/` |
| `into` | a folder the whole archive goes into (an app zipped without its `App/<Name>/`) |
| `map` | `[from, to]` prefixes for releases laid out otherwise (`["Games/", "Roms/PORTS/Games/"]`); the first match wins, the rest is left out |
| `skip` | card paths not installed though the release has them (`App/<Game>/config.json`: a game in Games → Ports only, not in Apps) |
| `shortcut` | `{ "launch": "App/X/launch.sh", "image": "App/X/icon.png" }`: Port Store makes an Onion Ports shortcut for a game released as an app |
| `keep` | files of the release the user changes (settings): installed once, then never overwritten or removed (prefix if it ends in `/`). Files the release does not have (saves, game data) are never touched anyway |
| `needs` | what the user brings: `what`, `hint`, `check` (paths that exist once it is in place), or a ROM by `sha1` looked for in `search` (`n64`: any byte order; `title`: header title, to tell another version of the same game). Game data checks are also read from the release's Onion shortcuts |

Build the index locally (without `GITHUB_TOKEN`, releases are read from github.com pages):

```sh
python3 index/build.py --out site/index.json --cache .cache
```

### Building Port Store

Rust 1.96 (`app/rust-toolchain.toml`) and the Miyoo Mini toolchain (GCC 8.3, glibc 2.28),
set up as in `../slot-mini/tools/mini/env.sh`.

```sh
tools/package.sh out https://l0st0npl4net.github.io/PortStore-Catalog-MiyooMini/index.json
# out/App/PortStore, as on the card
tools/release.sh https://l0st0npl4net.github.io/PortStore-Catalog-MiyooMini/index.json
# out/PortStore-<version>.zip, for a release tagged v<version> (version from app/Cargo.toml)
```

Port Store is in its own catalog (`recipes/portstore.json`), so a new release reaches the
devices as an update.

On the card: `App/PortStore/settings.json` (catalog address), `installed/` (what Port Store
put where, per port), `downloads/` (release files brought on a computer), `cache/` (log, last
catalog, pictures, ROM checksums).

Install is file by file: unpacked into a staging folder first, then moved into place; updates
remove only files the new version dropped; removal leaves `keep` files and anything Port
Store did not put there. After changes to `Roms/PORTS` shortcuts, Onion's `~Import ports`
runs when Port Store exits; `Roms/PORTS/Games` is made with them (the import reads a
shortcut's `GameDir` from there, `../../../App/<Game>` too, and Ports Collection does not
make it). On start, installed ports are brought in line with the catalog (files it now skips
go, shortcuts it now makes are made).

Commands for testing (also off the device, with `PORTSTORE_CARD`, `PORTSTORE_HOME`):
`portstore list | install ID | remove ID | needs ID | refresh | shots DIR [ID…]`.
