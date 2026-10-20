#!/usr/bin/env python3
"""The Rust crates built into Port Store, with their licenses (cargo metadata on stdin)."""
import json
import sys

m = json.load(sys.stdin)
used = {n["id"] for n in m["resolve"]["nodes"]}
print("Port Store is built with these Rust crates (name version: license):\n")
for p in sorted(m["packages"], key=lambda p: p["name"]):
    if p["id"] in used and p["name"] != "portstore":
        print(f"{p['name']} {p['version']}: {p['license']}")
