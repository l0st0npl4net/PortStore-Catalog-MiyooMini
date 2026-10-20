#!/bin/sh
# Make Port Store's release file, for a GitHub release of this repository that Port
# Store then offers as an update of itself:
#   tools/release.sh INDEX_URL
# writes out/PortStore-<version>.zip (version from app/Cargo.toml); the release's
# tag is v<version>.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
url=${1:?INDEX_URL}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/app/Cargo.toml" | head -n 1)
stage="$here/out/release"
rm -rf "${stage:?}"
"$here/tools/package.sh" "$stage" "$url"
zip="$here/out/PortStore-$version.zip"
rm -f "$zip"
# The install record is the device's own; the rest is what an update puts in place.
(cd "$stage" && python3 -I -c '
import os, sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as z:
    for root, dirs, files in os.walk("App"):
        dirs.sort()
        for f in sorted(files):
            p = os.path.join(root, f)
            if not p.startswith("App/PortStore/installed/"):
                z.write(p)
' "$zip")
echo "release file: $zip (tag v$version)"
