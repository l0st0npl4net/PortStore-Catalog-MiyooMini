#!/usr/bin/env python3
"""Put the published catalog (index.json and its pictures) into a folder, for
bundling with Port Store: a release always ships the catalog the devices get, never
a partial one built locally.
    python3 tools/fetch_catalog.py https://…/index.json OUT"""
import json
import sys
import urllib.request
from pathlib import Path

url, out = sys.argv[1], Path(sys.argv[2])
base = url.rsplit("/", 1)[0]
get = lambda u: urllib.request.urlopen(urllib.request.Request(u, headers={"User-Agent": "miyoo-ports"}), timeout=60).read()
data = get(url)
index = json.loads(data)
(out / "img").mkdir(parents=True, exist_ok=True)
(out / "index.json").write_bytes(data)
names = {x.get(k) for x in index["ports"] + index.get("manual", []) for k in ("thumb", "cover") if x.get(k)}
for name in sorted(names):
    if name.startswith("img/") and "/" not in name[4:] and ".." not in name:
        (out / name).write_bytes(get(f"{base}/{name}"))
print(f"catalog {index['generated']}: {len(index['ports'])} ports, {len(names)} pictures", file=sys.stderr)
