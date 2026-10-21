# Adding a port

A port gets into Port Store with a **recipe**: one JSON file in `recipes/ports/`, telling which
GitHub release file to take and what of it goes onto the SD card. Nothing is uploaded here:
devices download the file from its author's release.

A port listed in [MiyooMini-Ports](https://github.com/Producdevity/MiyooMini-Ports) whose
GitHub release is one archive laid out for the card (`App/<Name>/…` or `Roms/PORTS/…`, with
something that starts) needs no recipe: the catalog's build takes it by itself, marked `AUTO`.
A recipe is for everything else: another layout, files the user brings, settings to keep.

**Port authors:** if your port changes anything outside its own folders, ship a
`portstore-uninstall.sh` (or `mixtape-uninstall.sh`, the name Mixtape uses: one script for
both) anywhere in it. Before removing the port, Port Store runs it with `sh` in its folder,
with `PORTSTORE_UNINSTALL=1`, `MIXTAPE_UNINSTALL=1` and `SDCARD` (the card's root) set, for 30
seconds at most; the files go however it ends.

1. **Check the release.** It has to run on OnionOS on the Mini, as a release file (zip, tar,
   tar.gz or 7z). Look at its layout (`unzip -l file.zip`): `App/<Name>/…` (an app) or
   `Roms/PORTS/…` (a port for OnionOS's Ports Collection) work as they are.
2. **Write the recipe**, `recipes/ports/<id>.json`:
   ```json
   {
     "id": "my-port",
     "catalog": "My Port",
     "github": "author/my-port-miyoo",
     "asset": "^MyPort-OnionOS-v[\\d.]+\\.zip$"
   }
   ```
   - `catalog` is the port's name in
     [MiyooMini-Ports](https://github.com/Producdevity/MiyooMini-Ports) (`ports.json`): its
     categories, status and notes are used. Without an entry there, give `name`,
     `categories` (`app` for an app; a game's genre otherwise), `status`, `assets`.
   - `asset` matches the release file's name in every release (versions as `[\d.]+`).
   - Another layout: `into` (an app zipped without its `App/<Name>/` folder), or `map`
     (`[["Games/", "Roms/PORTS/Games/"], …]`).
   - Files the user brings (game data, a ROM): `needs`, with `what`, `hint` and the `check`
     paths that exist once they are in place.
   - All fields: [README, Recipes](README.md#recipes).
3. **Open a pull request.** A check builds the recipe as the catalog would and writes what it
   gives in the pull request: the release found, the files going onto the card, where the
   port shows (Games → MiyooPorts or Apps), what the user brings, whether it has pictures.
   Fix what it reports until it passes.

Once merged, the catalog is rebuilt and the port is in Port Store on every device at its next
start. How it all works: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

# Adding a porter's own catalog

A porter who publishes their games from their own site, with a catalog in the format of
[Mixtape](https://github.com/snyderman3000/mixtape) (`manifest.json`: each port's `name`,
`version`, `url` of a zip laid out for the card's root, `sha256`, `size`, `keep`…), needs no
recipe per game: one **source** file in `recipes/sources/` brings all of them.

```json
{
  "name": "slop.kitchen",
  "manifest": "https://slop.kitchen/mixtape/manifest.json"
}
```

- `name` is shown on each port's page as its source; `manifest` is the catalog's https address.
- `"dev": true` for a catalog of development builds: its ports are hidden until the user ticks
  development builds in the filter.
- Each port's id is its name in lower case with dashes (`LAST LIGHT` → `last-light`); it must
  not be the id of a recipe or of another catalog's port.
- Any other `version` in the catalog is an update on the devices, whatever it looks like; the
  file is checked against the catalog's `sha256` and `size`.

The same check runs on a pull request (`python3 index/check_recipes.py recipes/sources/<name>.json`
locally): every port of the catalog is built, and what each gives is listed.
