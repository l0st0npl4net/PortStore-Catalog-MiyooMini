#!/bin/sh
# MiyooPorts: the games Port Store installs. Each is a launcher made by Port
# Store (Roms/MiyooPorts/<Game>.sh), run from its own folder.
echo ":: MiyooPorts $*"
cd "$(dirname "$1")" || exit 1
sh "$1"
