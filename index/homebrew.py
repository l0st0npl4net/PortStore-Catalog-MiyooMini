#!/usr/bin/env python3
"""Build homebrew.json: the Game Boy, Game Boy Color, Game Boy Advance and NES homebrew
archived on Homebrew Hub (https://hh.gbdev.io), for Port Store's Homebrew tab.

What each entry is comes from Homebrew Hub's API (https://hh3.gbdev.io/api); its files
from the git repositories behind it (gbdev/database, gbadev-org/games,
nesdev-org/homebrew-db), read as one tree each at the commit of the build: each ROM's
size and git blob id (the SHA-1 of "blob <size>\\0" and its bytes), and an address
pinned to that commit, so what the device downloads is what was listed. Nothing is
downloaded here but the lists; nothing is hosted here.

Standard library only. GITHUB_TOKEN, when set, is used for the three tree requests.

    python3 index/homebrew.py --out site/homebrew.json [--previous URL|FILE]
"""
import argparse
import datetime
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://hh3.gbdev.io/api"
FORMAT = 1
# The system's folder in OnionOS (Roms/<folder>) by the ROM's extension; a dual-mode
# .gb cart goes to GB.
SYSTEMS = {".gba": "GBA", ".gbc": "GBC", ".cgb": "GBC", ".gb": "GB", ".nes": "NES"}
MAX_ROM = 32 << 20  # a GBA cart's size at most
SLUG = re.compile(r"^[a-z0-9][a-z0-9._-]*$")


def log(*a):
    print(*a, file=sys.stderr)


def get_json(url, token=None, tries=3):
    headers = {"User-Agent": "miyoo-ports-index", "Accept": "application/json"}
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    for attempt in range(tries):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as r:
                return json.load(r)
        except (urllib.error.URLError, TimeoutError, ValueError) as e:
            if attempt == tries - 1:
                raise RuntimeError(f"{url}: {e}") from e


def hub_entries():
    """Every entry of Homebrew Hub, in slug order (a stable order, so paging skips
    nothing). The API's own names for paging are sort, order and results."""
    out, page = [], 1
    while True:
        query = urllib.parse.urlencode({"sort": "slug", "order": "asc", "results": 50, "page": page})
        data = get_json(f"{API}/search?{query}")
        out += data.get("entries", [])
        if page >= data.get("page_total", 0) or not data.get("entries"):
            return out, data.get("results", len(out))
        page += 1


def names(value):
    """A field that may be text, a list of texts, an object with a name, or a list
    of such objects."""
    if isinstance(value, str):
        return [value.strip()] if value.strip() else []
    if isinstance(value, dict):
        return names(value.get("name"))
    if isinstance(value, list):
        return [n for v in value for n in names(v)]
    return []


def clip(text, n):
    text = " ".join(str(text).split())
    return text if len(text) <= n else text[: n - 1].rstrip() + "…"


def truthy(v):
    return v is True or (isinstance(v, str) and v.strip().lower() in ("true", "yes", "1"))


def safe_file(name):
    return isinstance(name, str) and name and not name.startswith("/") and "\\" not in name \
        and "://" not in name and all(p not in ("", ".", "..") for p in name.split("/"))


def rom_of(entry):
    """The ROM to install: as Homebrew Hub's own site picks it, the playable files (or
    all when none is marked), the default one first; one of a system Port Store
    knows."""
    files = [f for f in entry.get("files") or [] if isinstance(f, dict) and safe_file(f.get("filename"))]
    marked = any(f.get("playable") is True for f in files)
    playable = [f for f in files if not marked or f.get("playable") is True]
    for f in sorted(playable, key=lambda f: not f.get("default")):
        ext = os.path.splitext(f["filename"])[1].lower()
        if ext in SYSTEMS:
            return f["filename"], SYSTEMS[ext]
    return None, None


def date_of(value):
    """The release date as YYYY-MM-DD (or YYYY-MM, YYYY) from the free-form dates
    entries use; empty when it cannot be read."""
    if isinstance(value, (int, float)) and 1e8 < value < 1e10:
        return datetime.datetime.fromtimestamp(value, datetime.timezone.utc).strftime("%Y-%m-%d")
    if not isinstance(value, str):
        return ""
    s = value.strip()
    for fmt, out in (("%Y-%m-%dT%H:%M:%S", "%Y-%m-%d"), ("%Y-%m-%d", "%Y-%m-%d"), ("%Y/%m/%d", "%Y-%m-%d"),
                     ("%m-%d-%Y", "%Y-%m-%d"), ("%m/%d/%Y", "%Y-%m-%d"), ("%Y%m%d", "%Y-%m-%d"),
                     ("%Y-%m", "%Y-%m"), ("%Y", "%Y")):
        try:
            d = datetime.datetime.strptime(s[:19] if "T" in s else s, fmt)
            if 1989 <= d.year <= 2100:
                return d.strftime(out)
        except ValueError:
            pass
    return ""


def tree(repo, token):
    """(commit, {path: (blob sha1, size)}) of the repository's default branch."""
    info = get_json(f"https://api.github.com/repos/{repo}", token)
    commit = get_json(f"https://api.github.com/repos/{repo}/commits/{info['default_branch']}", token)["sha"]
    data = get_json(f"https://api.github.com/repos/{repo}/git/trees/{commit}?recursive=1", token)
    if data.get("truncated"):
        raise RuntimeError(f"{repo}: its tree is too large to read in one request")
    return commit, {t["path"]: (t["sha"], t["size"]) for t in data["tree"] if t.get("type") == "blob"}


def build(entries, trees):
    """The catalog's entries, and how many of Homebrew Hub's were left out (and why)."""
    out, left = [], {}

    def skip(why):
        left[why] = left.get(why, 0) + 1

    for e in entries:
        slug = e.get("slug")
        if not isinstance(slug, str) or not SLUG.match(slug):
            skip("no usable slug")
            continue
        if str(e.get("typetag", "")).lower() == "hackrom" or truthy(e.get("nsfw")):
            skip("hacks of commercial games, nsfw")
            continue
        rom, system = rom_of(e)
        if not rom:
            skip("no ROM for GB, GBC, GBA or NES")
            continue
        repo = re.sub(r"^https://github\.com/", "", str(e.get("baserepo", ""))).strip("/")
        if repo not in trees:
            skip("in a database not read")
            continue
        commit, files = trees[repo]
        found = files.get(f"entries/{slug}/{rom}")
        if not found:
            skip("ROM not in its database yet")
            continue
        sha1, size = found
        if not 0 < size <= MAX_ROM:
            skip("ROM of an odd size")
            continue
        raw = f"https://raw.githubusercontent.com/{repo}/{commit}/entries/{urllib.parse.quote(slug)}/"
        shot = next((s for s in names(e.get("screenshots")) if safe_file(s) and s.lower().endswith(".png")
                     and f"entries/{slug}/{s}" in files), None)
        item = {
            "slug": slug,
            "title": clip(e.get("title") or slug, 80),
            "system": system,
            "type": str(e.get("typetag") or "game").lower(),
            "devs": [clip(d, 60) for d in names(e.get("developer"))][:6],
            "tags": [clip(t, 30) for t in names(e.get("tags"))][:8],
            "license": clip((names(e.get("gameLicense")) or names(e.get("license")) or [""])[0], 40),
            "date": date_of(e.get("date")),
            "added": str(e.get("firstadded_date", ""))[:10],
            "desc": clip(e.get("description") or "", 600) if isinstance(e.get("description"), str) else "",
            "site": (names(e.get("website")) or names(e.get("repository")) or [""])[0],
            "file": {"url": raw + urllib.parse.quote(rom), "name": os.path.basename(rom), "size": size, "sha1": sha1},
        }
        if shot:
            item["shot"] = raw + urllib.parse.quote(shot)
        out.append({k: v for k, v in item.items() if v not in ("", [], None)})
    return out, left


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="site/homebrew.json")
    ap.add_argument("--previous", help="the published homebrew.json: kept when Homebrew Hub cannot be read")
    args = ap.parse_args()
    token = os.environ.get("GITHUB_TOKEN")
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    try:
        entries, total = hub_entries()
        repos = sorted({re.sub(r"^https://github\.com/", "", str(e.get("baserepo", ""))).strip("/")
                        for e in entries if str(e.get("baserepo", "")).startswith("https://github.com/")})
        trees = {repo: tree(repo, token) for repo in repos}
    except Exception as e:  # noqa: BLE001  (Homebrew Hub or GitHub down: keep what was published)
        log(f"homebrew: {e}")
        if args.previous:
            try:
                prev = json.load(urllib.request.urlopen(urllib.request.Request(
                    args.previous, headers={"User-Agent": "miyoo-ports-index"}), timeout=60)) \
                    if re.match(r"^https?://", args.previous) else json.load(open(args.previous, encoding="utf-8"))
                with open(args.out, "w", encoding="utf-8") as f:
                    json.dump(prev, f, ensure_ascii=False, separators=(",", ":"))
                log(f"homebrew: the previous list kept ({len(prev.get('entries', []))} entries)")
                return 0
            except Exception as e2:  # noqa: BLE001
                log(f"homebrew: previous list: {e2}")
        return 1
    items, left = build(entries, trees)
    out = {
        "format": FORMAT,
        "generated": now,
        "source": "Homebrew Hub (hh.gbdev.io)",
        "databases": {repo: commit for repo, (commit, _) in trees.items()},
        "entries": sorted(items, key=lambda i: i["slug"]),
    }
    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, separators=(",", ":"))
    log(f"homebrew: {len(items)} of {total} entries -> {args.out}")
    for why, n in sorted(left.items(), key=lambda kv: -kv[1]):
        log(f"      left out: {n:4} {why}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
