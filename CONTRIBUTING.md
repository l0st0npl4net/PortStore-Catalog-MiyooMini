# Adding a port

A port gets into Port Store with a **recipe**: one JSON file in `recipes/ports/`, telling which
GitHub release file to take and what of it goes onto the SD card. Nothing is uploaded here:
devices download the file from its author's release.

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
