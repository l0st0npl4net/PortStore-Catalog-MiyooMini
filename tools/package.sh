#!/bin/sh
# Build Port Store for the Miyoo Mini and lay it out as on the SD card:
#   tools/package.sh OUT [INDEX_URL]
# OUT/App/PortStore is then copied to the card's App folder. INDEX_URL is where the
# catalog is published (GitHub Pages of this repository); site/index.json, when built
# (index/build.py), goes in as the catalog to start with.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
out=${1:?OUT}
url=${2:-${PORTSTORE_INDEX_URL:-}}
. "$here/../slot-mini/tools/mini/env.sh"
(cd "$here/app" && cargo build --release --target armv7-unknown-linux-gnueabihf)
app="$out/App/PortStore"
rm -rf "${app:?}"
mkdir -p "$app/licenses"
cp "$here/app/target/armv7-unknown-linux-gnueabihf/release/portstore" "$app/"
cp "$here/app/dist/launch.sh" "$here/app/dist/config.json" "$here/assets/icon.png" "$app/"
chmod +x "$app/launch.sh" "$app/portstore"
# On the Mini v4 and Flip, OnionOS switches to the panel's own 752x560 for an app
# with this file; Port Store lays itself out for the screen it is given.
: > "$app/full_resolution"
printf '{\n  "index": "%s"\n}\n' "$url" > "$app/settings.json"
# Mozilla's root certificates, for checking the servers (curl --cacert).
cp /etc/ssl/certs/ca-certificates.crt "$app/cacert.pem"
# The published catalog and its pictures, as the devices get it (for a Mini without
# Wi-Fi; newer ones are downloaded). PORTSTORE_LOCAL_CATALOG=1 takes site/ instead.
if [ -n "$url" ] && [ -z "${PORTSTORE_LOCAL_CATALOG:-}" ]; then
    python3 -I "$here/tools/fetch_catalog.py" "$url" "$app"
elif [ -f "$here/site/index.json" ]; then
    cp "$here/site/index.json" "$app/index.json"
    [ -d "$here/site/img" ] && cp -r "$here/site/img" "$app/img"
fi
cp "$here/LICENSE" "$app/licenses/PortStore.txt"
cp "$here/assets/DejaVu_License.txt" "$app/licenses/DejaVu.txt"
(cd "$here/app" && cargo metadata --format-version 1 --filter-platform armv7-unknown-linux-gnueabihf) |
    python3 -I "$here/tools/crate_licenses.py" > "$app/licenses/rust-crates.txt"
cat > "$app/licenses/cacert.txt" <<'TXT'
cacert.pem: Mozilla's CA certificate list (as Debian's ca-certificates ships it),
under the Mozilla Public License 2.0: https://www.mozilla.org/MPL/2.0/
TXT
mkdir -p "$app/import"
cat > "$app/import/README.txt" <<'TXT'
Game files from a computer (Undertale's, Fallout's, a ROM...) can be copied here as
they are, whole folders too. On a port's page Port Store shows what it found here for
that port and moves it into place with Start.
TXT
mkdir -p "$app/downloads"
cat > "$app/downloads/README.txt" <<'TXT'
No Wi-Fi? This folder is how Port Store gets files from a computer.

1. In Port Store, try to install (or update) a port. Without a connection it writes
   to-download.txt here: the address of the newest catalog and of each file wanted.
2. On a computer, download them into this folder (App/PortStore/downloads on the
   card), each under the name given in to-download.txt. Save the catalog as
   index.json.
3. Back in Port Store: the newer catalog is used at once (its Updates tab too), and
   Install takes the file from here.

A file stays here after installing: delete it to free space.
TXT
# Port Store's own install record, so the catalog's Port Store shows as installed
# and its updates as updates (tools/release.sh makes the release of the same version).
version=v$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/app/Cargo.toml" | head -n 1)
mkdir -p "$app/installed"
(cd "$out" && find App/PortStore -type f ! -path "App/PortStore/installed/*" | sort) | python3 -I -c '
import json, sys
files = [l.strip() for l in sys.stdin if l.strip()]
print(json.dumps({"id": "portstore", "version": sys.argv[1], "date": sys.argv[2], "files": files,
                  "shortcuts": False}, indent=1))
' "$version" "$(date -u +%Y-%m-%d)" > "$app/installed/portstore.json"
echo "packaged: $app ($version)"
