#!/bin/sh
# Port Store: installs ports from their authors' own releases.
cd "$(dirname "$0")"
mkdir -p cache
[ -f cache/portstore.log ] && mv -f cache/portstore.log cache/portstore.log.1
# OnionOS's curl and its libraries do the downloads.
export LD_LIBRARY_PATH="/mnt/SDCARD/.tmp_update/lib:$LD_LIBRARY_PATH"
./portstore > cache/portstore.log 2>&1
# New or removed shortcuts for Games > Ports: OnionOS's ~Import ports shows the ones
# whose game files are in place.
if [ -f cache/import ]; then
    rm -f cache/import
    [ -f /mnt/SDCARD/Emu/PORTS/import.sh ] && sh /mnt/SDCARD/Emu/PORTS/import.sh
fi
sync
