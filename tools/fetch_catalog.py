#!/usr/bin/env python3
"""Put the published catalog (index.json and its pictures, homebrew.json when
published) into a folder, for bundling with Port Store: a release always ships the
catalog the devices get, never a partial one built locally.
    python3 tools/fetch_catalog.py https://…/index.json OUT"""
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path

url, out = sys.argv[1], Path(sys.argv[2])
base = url.rsplit("/", 1)[0]
def get(u, tries=3):
    """The file at `u`, tried again on a network hiccup (a handshake timing out)."""
    for attempt in range(tries):
        try:
            return urllib.request.urlopen(urllib.request.Request(u, headers={"User-Agent": "miyoo-ports"}), timeout=60).read()
        except urllib.error.HTTPError:
            raise
        except OSError:
            if attempt == tries - 1:
                raise



data = get(url)
index = json.loads(data)
(out / "img").mkdir(parents=True, exist_ok=True)
(out / "index.json").write_bytes(data)
names = {x.get(k) for x in index["ports"] + index.get("dev", []) + index.get("manual", [])
         for k in ("thumb", "cover") if x.get(k)}
for name in sorted(names):
    if name.startswith("img/") and "/" not in name[4:] and ".." not in name:
        (out / name).write_bytes(get(f"{base}/{name}"))
print(f"catalog {index['generated']}: {len(index['ports'])} ports, {len(names)} pictures", file=sys.stderr)
try:
    (out / "homebrew.json").write_bytes(get(f"{base}/homebrew.json"))
except OSError as e:  # not published yet: the device downloads it when the tab is opened
    print(f"homebrew.json: {e}", file=sys.stderr)
