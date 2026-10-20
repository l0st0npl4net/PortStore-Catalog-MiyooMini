#!/usr/bin/env python3
"""Drop downloads from the cache that the index no longer names.
    python3 index/prune_cache.py site/index.json .cache"""
import hashlib
import json
import sys
from pathlib import Path

index = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
keep = {hashlib.sha256(p["file"]["url"].encode()).hexdigest()[:24] for p in index["ports"]}
for f in Path(sys.argv[2]).iterdir():
    if f.is_file() and f.name not in keep:
        f.unlink()
