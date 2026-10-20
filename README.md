# miyoo-ports

Port Store: install and update ports on a Miyoo Mini / Mini Plus running Onion, right on
the device, from the ports' own GitHub releases.

- `recipes/` — one JSON file per port: which release file to take and what of it goes onto
  the SD card.
- `index/build.py` — builds `index.json` from the recipes (GitHub Actions, daily →
  GitHub Pages). Catalog data (categories, status, notes) comes from
  [Producdevity/MiyooMini-Ports](https://github.com/Producdevity/MiyooMini-Ports); its
  entries without a recipe are listed as links.
- `app/` — Port Store itself (Rust; draws to the framebuffer, no SDL).

Nothing is re-hosted: the device downloads each file from its author's release and checks
it against the SHA-256 in the index. Only the ports' pictures are published with the index
(`img/`, made small by the build: 48×48 for the list, up to 224×168 for a port's page),
taken from the release (Onion's `Roms/PORTS/Imgs`, or an app's `icon.png`) or else from
the catalog's `image`. The build needs Pillow for them (`pip install pillow`); without it
the index is built without pictures.

## Recipes

```json
{
  "id": "mario-kart-64",
  "name": "Mario Kart 64",
  "github": "mr-finster/miyoo64",
  "asset": "^MarioKart64-miyoo\\.zip$",
  "take": ["App/MarioKart64/", "Roms/PORTS/"],
  "keep": ["App/MarioKart64/rom.z64", "App/MarioKart64/data/"],
  "needs": [{ "what": "ROM: Mario Kart 64 (USA)", "sha1": "579c…", "search": ["Roms/N64"], "n64": true }]
}
```

| Field | |
|---|---|
| `id` | unique, also the name of the install record on the card |
| `catalog` | the entry's `name` in MiyooMini-Ports `ports.json`: its metadata is used |
| `name`, `group`, `categories`, `status`, `assets`, `porter`, `about`, `url`, `image` | override or replace catalog metadata. Category `app` puts a port under Apps, the rest under Games; ports of a `group` are listed together under its heading |
| `github`, `asset`, `tag`, `prerelease` | release search: repository, file name regex, tag regex, allow pre-releases |
| `into` | a folder the whole archive goes into (an app zipped without its `App/<Name>/`) |
| `strip` | prefix taken off every member (default: a single wrapper folder that is not a card folder) |
| `take` | path prefixes (from the card's root) that are installed; default `App/`, `Roms/PORTS/` |
| `keep` | files of the release the user changes (settings): installed once, then never overwritten or removed (prefix if it ends in `/`). Files the release does not have (saves, game data) are never touched anyway |
| `needs` | what the user brings: `what`, `hint`, `check` (paths that exist once it is in place), or a ROM by `sha1` looked for in `search` (`n64`: any byte order; `title`: header title, to tell another version of the same game) |

Build locally (60 GitHub API requests an hour without a token):

```sh
GITHUB_TOKEN=… python3 index/build.py --out site/index.json --cache .cache
```

## Device

```sh
tools/package.sh out https://<user>.github.io/miyoo-ports/index.json
# copy out/App/PortStore to the card's App folder
```

On the card: `App/PortStore/settings.json` (catalog address), `installed/` (what Port
Store put where, per port), `downloads/` (release files brought on a computer, for a Mini
without Wi-Fi), `cache/` (log, last catalog, ROM checksums).

Install is file by file: unpacked into a staging folder first, then moved into place;
updates remove only files the new version dropped; removal leaves `keep` files and
anything Port Store did not put there. After changes to `Roms/PORTS` shortcuts, Onion's
`~Import ports` runs when Port Store exits.

Commands for testing (also off the device, with `PORTSTORE_CARD`, `PORTSTORE_HOME`):
`portstore list | install ID | remove ID | needs ID | refresh | shots DIR [ID…]`.
