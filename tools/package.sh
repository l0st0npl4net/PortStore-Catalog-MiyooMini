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
printf '{\n  "index": "%s"\n}\n' "$url" > "$app/settings.json"
# Mozilla's root certificates, for checking the servers (curl --cacert).
cp /etc/ssl/certs/ca-certificates.crt "$app/cacert.pem"
[ -f "$here/site/index.json" ] && cp "$here/site/index.json" "$app/index.json"
cp "$here/LICENSE" "$app/licenses/PortStore.txt"
cp "$here/assets/DejaVu_License.txt" "$app/licenses/DejaVu.txt"
(cd "$here/app" && cargo metadata --format-version 1 --filter-platform armv7-unknown-linux-gnueabihf) |
    python3 -I "$here/tools/crate_licenses.py" > "$app/licenses/rust-crates.txt"
cat > "$app/licenses/cacert.txt" <<'TXT'
cacert.pem: Mozilla's CA certificate list (as Debian's ca-certificates ships it),
under the Mozilla Public License 2.0: https://www.mozilla.org/MPL/2.0/
TXT
mkdir -p "$app/downloads"
cat > "$app/downloads/README.txt" <<'TXT'
No Wi-Fi? Download a port's release file on a computer (the file named on the
port's page in Port Store) and put it here: Port Store installs it from here.
TXT
echo "packaged: $app"
