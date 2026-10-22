#!/usr/bin/env python3
"""Build index.json, the catalog Port Store reads on the device.

For each recipe (recipes/**/*.json) it finds the newest release of the port's GitHub
repository with a matching file, downloads that file, checks what is inside and which
part of it goes onto the SD card, and writes the file's address, size and SHA-256 with
the install rules. Porters' own catalogs in Mixtape's format (recipes/sources/*.json
says where) give their ports the same way, each from the file the catalog names.
Catalog entries without a recipe (Producdevity/MiyooMini-Ports ports.json) are listed
as "manual": a link only.

Standard library only. GITHUB_TOKEN, when set, lifts the API limit (60 requests an hour
without it).

    python3 index/build.py --out site/index.json [--previous URL|FILE] [--cache DIR]
"""
import argparse
import datetime
import hashlib
import io
import json
import os
import posixpath
import re
import sys
import tarfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path, PurePosixPath

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
# HEAD: the repository's default branch, whatever it is called (master there).
CATALOG_URL = "https://raw.githubusercontent.com/Producdevity/MiyooMini-Ports/HEAD/ports.json"
# Top folders of an OnionOS SD card; anything else at the top of an archive is a wrapper
# folder (stripped when it is the only one) or files for other systems.
CARD_ROOTS = {"App", "Roms", "Emu", "RApp", "BIOS", "Saves", "Themes", "Icons", "Media"}
DEFAULT_TAKE = ["App/", "Roms/PORTS/"]
FORMAT = 1


def log(*a):
    print(*a, file=sys.stderr)


class Github:
    def __init__(self, token):
        self.token = token
        self.cache = {}

    def get(self, url):
        req = urllib.request.Request(url, headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "miyoo-ports-index",
            **({"Authorization": f"Bearer {self.token}"} if self.token else {}),
        })
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.load(r)

    def releases(self, repo):
        if repo not in self.cache:
            if self.token:
                self.cache[repo] = self.get(f"https://api.github.com/repos/{repo}/releases?per_page=20")
            else:
                self.cache[repo] = self.web_releases(repo)
        return self.cache[repo]

    def page(self, url):
        req = urllib.request.Request(url, headers={"User-Agent": "miyoo-ports-index"})
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.read().decode("utf-8", "replace")

    def web_releases(self, repo, tags=5):
        """Without a token: the releases feed and asset lists of github.com, which the
        API limit does not count. Pre-releases are not told apart there; sizes and
        digests are not given (the file is measured once downloaded)."""
        feed = self.page(f"https://github.com/{repo}/releases.atom")
        out = []
        for entry in re.findall(r"<entry>(.*?)</entry>", feed, re.S)[:tags]:
            link = re.search(r'<link[^>]*href="[^"]*/releases/tag/([^"]+)"', entry)
            updated = re.search(r"<updated>([^<]+)</updated>", entry)
            if not link:
                continue
            tag = urllib.parse.unquote(link.group(1))
            html = self.page(f"https://github.com/{repo}/releases/expanded_assets/{urllib.parse.quote(tag)}")
            quoted_tag = re.escape(urllib.parse.quote(tag, safe="/"))
            names = re.findall(rf'href="/{re.escape(repo)}/releases/download/{quoted_tag}/([^"/]+)"', html)
            assets = [{"name": urllib.parse.unquote(n),
                       "browser_download_url":
                           f"https://github.com/{repo}/releases/download/{urllib.parse.quote(tag, safe='/')}/{n}",
                       "size": None} for n in dict.fromkeys(names)]
            out.append({"tag_name": tag, "published_at": updated.group(1) if updated else "", "assets": assets})
        return out


def pick_release(releases, recipe):
    """The newest release with a file matching the recipe's `asset`, and the tags of
    newer releases passed over for having none (a release published without its
    file: worth a warning)."""
    asset_re = re.compile(recipe["asset"])
    tag_re = re.compile(recipe.get("tag", ""))
    passed = []
    for rel in sorted(releases, key=lambda r: r.get("published_at") or "", reverse=True):
        if rel.get("draft") or (rel.get("prerelease") and not recipe.get("prerelease")):
            continue
        if not tag_re.search(rel.get("tag_name", "")):
            continue
        for asset in rel.get("assets", []):
            if asset_re.search(asset["name"]):
                return rel, asset, passed
        passed.append(rel.get("tag_name", "?"))
    return None, None, passed


MAX_DOWNLOAD = 2 << 30  # no port is anywhere near; a bigger file is refused
# Names in the cache this build used: the rest is dropped at the end of a full build.
USED = set()


def cache_name(asset):
    """The cache's name for a release file: by GitHub's own id and upload time when
    known (a file uploaded again under the same name and size is another file), else
    by its address."""
    key = asset["browser_download_url"]
    if asset.get("id") is not None:
        key += f"#{asset['id']}@{asset.get('updated_at', '')}"
    return hashlib.sha256(key.encode()).hexdigest()[:24]


def download(asset, cache, fresh=False):
    """The release file, from the cache when it is there at the expected size."""
    url, size = asset["browser_download_url"], asset.get("size")
    path = cache / cache_name(asset)
    USED.add(path.name)
    if not fresh and path.exists() and size in (None, path.stat().st_size):
        return path
    tmp = path.with_suffix(".part")
    req = urllib.request.Request(url, headers={"User-Agent": "miyoo-ports-index"})
    got = 0
    with urllib.request.urlopen(req, timeout=300) as r, open(tmp, "wb") as f:
        while chunk := r.read(1 << 20):
            got += len(chunk)
            if got > MAX_DOWNLOAD:
                raise ValueError(f"{url}: larger than {MAX_DOWNLOAD >> 20} MB")
            f.write(chunk)
    if size is not None and got != size:
        tmp.unlink()
        raise ValueError(f"{url}: {got} bytes downloaded, {size} expected")
    tmp.rename(path)
    return path


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


def kind_of(name):
    low = name.lower()
    for ext, kind in ((".zip", "zip"), (".tar.gz", "tar.gz"), (".tgz", "tar.gz"), (".tar", "tar"), (".7z", "7z")):
        if low.endswith(ext):
            return kind
    return None


def tar_name(name):
    """A tar member's name as the device reads it: without leading "./"."""
    while name.startswith("./"):
        name = name[2:]
    return "" if name == "." else name


def sniff(path):
    """The archive's kind by its first bytes: names lie (a ".tar.gz" that is a tar)."""
    with open(path, "rb") as f:
        head = f.read(512)
    if head.startswith(b"PK\x03\x04") or head.startswith(b"PK\x05\x06"):
        return "zip"
    if head.startswith(b"\x1f\x8b"):
        return "tar.gz"
    if head[257:262] == b"ustar":
        return "tar"
    if head.startswith(b"7z\xbc\xaf\x27\x1c"):
        return "7z"
    return None


def seven(path):
    """A 7z archive, read with py7zr (pip install py7zr): the device unpacks it with
    OnionOS's 7-Zip."""
    import py7zr  # only for 7z releases

    return py7zr.SevenZipFile(path)


def entries(path, kind):
    """(name, size, is_dir, is_link, unreadable) of every member; `unreadable` says
    why Port Store on the device could not read it (None when it can)."""
    if kind == "7z":
        with seven(path) as z:
            return [(f.filename + ("/" if f.is_directory else ""), f.uncompressed or 0, f.is_directory,
                     bool(f.is_symlink), None) for f in z.files]
    if kind == "zip":
        with zipfile.ZipFile(path) as z:
            out = []
            for i in z.infolist():
                link = (i.external_attr >> 16) & 0o170000 == 0o120000
                why = None
                if i.flag_bits & 1:
                    why = "encrypted"
                elif i.compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED):
                    why = f"compression method {i.compress_type} (Port Store reads stored and deflate)"
                elif "\0" in i.orig_filename:
                    why = "a NUL in its name"
                out.append((dos_name(i), i.file_size, i.is_dir() or i.filename.endswith("\\"), link, why))
            return out
    with tarfile.open(path, "r:gz" if kind == "tar.gz" else "r:") as t:
        return [(tar_name(m.name) + ("/" if m.isdir() else ""), m.size, m.isdir(), m.issym() or m.islnk(), None)
                for m in t.getmembers()
                if (m.isfile() or m.isdir() or m.issym() or m.islnk()) and tar_name(m.name)]


def dos_name(info):
    """A zip member's name as Port Store on the device reads it (archive::zip_name):
    UTF-8 when flagged, else UTF-8 when the bytes are valid UTF-8, else code page 437;
    zips made on Windows have their backslashes read as folders, as unzip does."""
    name = info.orig_filename
    if not info.flag_bits & 0x800:
        try:
            name = name.encode("cp437").decode("utf-8")
        except (UnicodeEncodeError, UnicodeDecodeError):
            pass
    name = name.split("\0", 1)[0]
    if info.is_dir() and not name.endswith("/"):
        name += "/"
    return name.replace("\\", "/") if info.create_system == 0 else name


def read_member(path, kind, name, limit=16 << 20):
    """A member's bytes, refused past `limit` (by its declared size, before reading:
    an archive is untrusted)."""
    def check(size):
        if size > limit:
            raise ValueError(f"{name}: {size >> 20} MB, more than a picture or shortcut can be")
    if kind == "7z":
        with seven(path) as z:
            info = next(f for f in z.files if f.filename == name)
            check(info.uncompressed or 0)
        with seven(path) as z:
            return next(iter(z.read([name]).values())).read()
    if kind == "zip":
        with zipfile.ZipFile(path) as z:
            info = next(i for i in z.infolist() if dos_name(i) == name)
            check(info.file_size)
            with z.open(info) as f:
                return f.read(limit + 1)[:limit]
    with tarfile.open(path, "r:gz" if kind == "tar.gz" else "r:") as t:
        member = next(m for m in t.getmembers() if tar_name(m.name) == name)
        check(member.size)
        f = t.extractfile(member)
        return f.read() if f else b""


def shortcut_checks(path, kind, origin, files):
    """The game files OnionOS's ~Import ports looks for (GameDir, GameDataFile in each
    .port shortcut) that the release does not bring: the user's to put in place."""
    out = []
    for f in files:
        if not (f.startswith("Roms/PORTS/Shortcuts/") and f.endswith((".port", ".notfound"))):
            continue
        text = read_member(path, kind, origin[f], limit=1 << 20).decode("utf-8", "replace")
        field = lambda key: (re.findall(rf'^\s*{key}="([^"]*)"', text, re.M) or [""])[0]
        game_dir = field("GameDir") or field("RomDir")
        data = field("GameDataFile") or field("GameExecutable") or field("RomFile")
        if not game_dir or not data:
            continue
        need = posixpath.normpath(f"Roms/PORTS/Games/{game_dir}/{data}")
        if need not in files and not need.startswith(".."):
            out.append(need)
    return sorted(set(out))


def needs_of(recipe, checks):
    """The recipe's needs, with the shortcuts' checks for those that have none."""
    needs = [dict(n) for n in recipe.get("needs", [])]
    if not checks:
        return needs
    for n in needs:
        if not n.get("check") and not n.get("sha1"):
            n["check"] = checks
            return needs
    if not any(n.get("check") for n in needs):
        folder = posixpath.dirname(checks[0])
        needs.append({"what": "The game's own files",
                      "hint": f"Put them in {folder}: see the port's page for which.",
                      "check": checks})
    return needs


def safe(name):
    p = PurePosixPath(name)
    return not p.is_absolute() and ".." not in p.parts and "\\" not in name


def wrapper(names):
    """The single top folder all members are in, when it is not a card folder."""
    tops = {n.split("/", 1)[0] for n in names if n.strip("/")}
    if len(tops) == 1:
        top = next(iter(tops))
        if top not in CARD_ROOTS and any("/" in n.strip("/") for n in names):
            return top + "/"
    return ""


def target(recipe, rel):
    """Where a member (its path past `strip`) goes on the card, if anywhere: into one
    folder (`into`), by the first matching [from, to] of `map`, or as it is when
    under `take`. The device does the same (install.rs, target)."""
    if recipe.get("into"):
        return recipe["into"] + rel
    if recipe.get("map"):
        return next((to + rel[len(src):] for src, to in recipe["map"] if rel.startswith(src)), None)
    return rel if any(rel.startswith(t) for t in recipe.get("take", DEFAULT_TAKE)) else None


def plan(members, recipe):
    """What of the archive goes onto the card: stripped prefix, files (card paths),
    bytes, what is left out, and each file's member name."""
    names = [m[0] for m in members]
    strip = recipe.get("strip", wrapper(names))
    take = recipe.get("take", DEFAULT_TAKE)
    skip = recipe.get("skip", [])
    files, size, skipped = [], 0, set()
    origin = {}
    for name, length, is_dir, link, unreadable in members:
        if not name.startswith(strip) or is_dir:
            continue
        rel = name[len(strip):]
        to = target(recipe, rel)
        if to is not None and not any(to.startswith(k) for k in skip):
            # Only what goes onto the card has to be safe.
            if not safe(name):
                raise ValueError(f"unsafe path in archive: {name}")
            if link:
                raise ValueError(f"link in archive: {name}")
            if unreadable:
                raise ValueError(f"{name}: {unreadable}")
            files.append(to)
            origin[to] = name
            size += length
        else:
            parts = rel.split("/")
            skipped.add("/".join(parts[:2]) + ("/" if len(parts) > 2 else ""))
    if not files:
        raise ValueError(f"nothing under {take} (top: {sorted(skipped)[:6]})")
    return strip, files, size, sorted(skipped), origin


def roots(files, nested=False):
    """The folders the port owns, App/<Name>/ (Emu/, RApp/) and Roms/PORTS/Games/<Name>/: how the
    device tells a port put on the card by hand. `nested`: a game on a runtime (one that
    `requires` another) owns Roms/PORTS/Games/<Runtime>/games/<Name>/, not the runtime's folder."""
    out = set()
    for f in files:
        p = f.split("/")
        if p[0] in ("App", "Emu", "RApp") and len(p) > 2:
            out.add(f"{p[0]}/{p[1]}/")
        elif nested and p[:3] == ["Roms", "PORTS", "Games"] and len(p) > 6 and p[4] == "games":
            out.add("/".join(p[:6]) + "/")
        elif p[:3] == ["Roms", "PORTS", "Games"] and len(p) > 4:
            out.add("/".join(p[:4]) + "/")
    return sorted(out)


def keeps(rule, path):
    """As the device's index::keeps: a folder's rule when it ends in '/', a pattern
    when it has '*' (any characters but '/') or '?' (one), else that file."""
    if rule.endswith("/"):
        return path.startswith(rule)
    if "*" not in rule and "?" not in rule:
        return path == rule
    pattern = "".join("[^/]*" if c == "*" else "[^/]" if c == "?" else re.escape(c) for c in rule)
    return re.fullmatch(pattern, path) is not None


def marks(files, keep, nested=False):
    """A file of the port's in each of its folders (not one the user keeps): all on
    the card, the port is there."""
    kept = lambda f: any(keeps(k, f) for k in keep)
    out = []
    for root in roots(files, nested):
        mine = sorted(f for f in files if f.startswith(root) and not kept(f))
        if mine:
            out.append(min(mine, key=lambda f: (f.count("/"), f)))
    return out


def load_recipes():
    """Every recipe, with its file; a recipe that cannot be read, or whose id another
    recipe has too, comes with the reason it fails (`_error`), the rest still build."""
    out = []
    for path in sorted((ROOT / "recipes").rglob("*.json")):
        if path.parent == SOURCES:
            continue
        rel = str(path.relative_to(ROOT))
        try:
            r = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(r, dict) or not isinstance(r.get("id"), str):
                raise ValueError("not a recipe (no `id`)")
        except ValueError as e:
            out.append({"id": path.stem, "_file": rel, "_error": f"cannot read it: {e}"})
            continue
        r["_file"] = rel
        out.append(r)
    files = {}
    for r in out:
        files.setdefault(r["id"], []).append(r["_file"])
    for r in out:
        others = [f for f in files[r["id"]] if f != r["_file"]]
        if others and "_error" not in r:
            r["_error"] = f"id {r['id']} is also the id of {', '.join(others)}"
    return out


SOURCES = ROOT / "recipes" / "sources"
# A Mixtape catalog's fields Port Store takes, with their types.
MIXTAPE = {"name": str, "version": str, "url": str, "sha256": str, "size": int, "keep": "texts", "requires": "texts",
           "needs": list}
# What a catalog's need may say (as a recipe's: check_recipes.NEED_FIELDS), with its types.
MIXTAPE_NEED = {"what": str, "hint": str, "check": "texts", "sha1": str, "search": "texts", "n64": bool, "title": str}


def mixtape_needs(needs):
    """A catalog entry's `needs` (a game the user brings: what, hint, check...) as a
    recipe's, or the reason they are not."""
    for n in needs:
        if not isinstance(n, dict) or not isinstance(n.get("what"), str) or not n["what"]:
            return "each of `needs` is an object that says `what`"
        if bad := [k for k in n if k not in MIXTAPE_NEED or not typed(n[k], MIXTAPE_NEED[k])]:
            return f"`needs`: unknown or mistyped {', '.join(bad)}"
        paths = n.get("check", []) + n.get("search", [])
        if not all(p and not p.startswith("/") and "\\" not in p and ".." not in p.split("/") for p in paths):
            return "`needs`: paths are relative to the card, without `..`"
    return None


def source_recipes(paths=None, previous=None, today=None):
    """Recipes for the ports of porters' own catalogs (Mixtape's format), one source
    file each in recipes/sources: {"name", "manifest", "dev"?}. A port's id is its
    name's slug; its date the day its version was first seen. A catalog that cannot
    be read comes as one recipe with `_error` and `_source`: its ports are kept from
    the previous index."""
    if paths is None:
        paths = sorted(SOURCES.glob("*.json")) if SOURCES.is_dir() else []
    previous = previous or {}
    today = today or datetime.date.today().isoformat()
    out = []
    for path in map(Path, paths):
        rel = str(path.resolve().relative_to(ROOT)) if path.resolve().is_relative_to(ROOT) else str(path)
        src = {}
        try:
            src = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(src, dict) or not all(isinstance(src.get(k), str) for k in ("name", "manifest")):
                src = {}
                raise ValueError("a source has `name` and `manifest` (texts)")
            if not isinstance(src.get("dev", False), bool):
                raise ValueError("`dev` is true or false")
            if not src["manifest"].startswith("https://"):
                raise ValueError("`manifest` is an https address")
            manifest = load_json(src["manifest"])
            listed = manifest.get("ports") if isinstance(manifest, dict) else None
            if not isinstance(listed, list):
                raise ValueError(f"{src['manifest']}: no `ports` list")
        except Exception as e:  # noqa: BLE001  (a host down, a bad file: this source only)
            out.append({"id": path.stem, "_file": rel, "_error": f"source: {e}", "_source": src.get("name", path.stem),
                        "_dev": src.get("dev") is True, "_whole": True})
            continue
        for m in listed:
            name = m.get("name") if isinstance(m, dict) else None
            rid = slug(name) if isinstance(name, str) else ""
            recipe = {"id": rid or f"{path.stem}-entry", "_file": rel, "_source": src["name"],
                      "_dev": src.get("dev", False)}
            if not isinstance(m, dict) or not rid or not isinstance(m.get("url"), str) \
                    or not isinstance(m.get("version"), str) or not m["version"].strip():
                recipe["_error"] = "a catalog entry has `name`, `version` and `url`"
            elif bad := [k for k, kind in MIXTAPE.items() if k in m and not typed(m[k], kind)]:
                recipe["_error"] = f"wrong type of {', '.join(bad)}"
            elif not m["url"].startswith("https://"):
                recipe["_error"] = "the file's address is not https"
            elif reason := mixtape_needs(m.get("needs", [])):
                recipe["_error"] = reason
            else:
                old = previous.get(rid, {})
                recipe.update({"_mixtape": m, "name": name, "keep": m.get("keep", []), "requires": m.get("requires", []),
                               "needs": m.get("needs", []),
                               "_date": old.get("date", today) if old.get("version") == m["version"] else today})
            out.append(recipe)
    return out


def load_json(source):
    if not source:
        return None
    if re.match(r"^https?://", source):
        req = urllib.request.Request(source, headers={"User-Agent": "miyoo-ports-index"})
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.load(r)
    return json.loads(Path(source).read_text(encoding="utf-8"))


META = ("name", "categories", "status", "assets", "porter", "notes", "image", "url")
# Pictures as the device shows them: in the list, and on a port's page.
SIZES = {"thumb": (48, 48), "cover": (224, 168)}


def archive_image(path, kind, origin, files):
    """The picture the release brings: OnionOS's for Games > Ports (Roms/PORTS/Imgs),
    else an app's icon."""
    for prefix in ("Roms/PORTS/Imgs/", "App/"):
        found = sorted(f for f in files if f.startswith(prefix) and f.lower().endswith(".png")
                       and (prefix != "App/" or f.lower().endswith("/icon.png")))
        if found:
            return read_member(path, kind, origin[found[0]])
    return None


def fetch(url, cache):
    path = cache / ("img-" + hashlib.sha256(url.encode()).hexdigest()[:24])
    USED.add(path.name)
    if not path.exists():
        req = urllib.request.Request(url, headers={"User-Agent": "miyoo-ports-index"})
        with urllib.request.urlopen(req, timeout=60) as r:
            path.write_bytes(r.read(16 << 20))
    return path.read_bytes()


def pictures(key, data, site):
    """The picture in the sizes the device shows, as PNG files under site/img; their
    names (with a hash, so the device's cache is never stale) by size."""
    from PIL import Image  # only here: the rest of the build works without Pillow

    Image.MAX_IMAGE_PIXELS = 40_000_000
    image = Image.open(io.BytesIO(data))
    image.load()
    image = image.convert("RGBA")
    out = {}
    for kind, size in SIZES.items():
        small = image.copy()
        small.thumbnail(size, Image.LANCZOS)
        buf = io.BytesIO()
        small.save(buf, "PNG", optimize=True)
        name = f"img/{key}-{kind}-{hashlib.sha256(buf.getvalue()).hexdigest()[:8]}.png"
        (site / name).parent.mkdir(parents=True, exist_ok=True)
        (site / name).write_bytes(buf.getvalue())
        out[kind] = name
    return out


def add_pictures(item, key, data_of, site):
    """Put `item`'s pictures in `site` from the first source that has one; a picture
    failing is no reason to leave the port out."""
    for source in data_of:
        try:
            data = source()
            if data:
                item.update(pictures(key, data, site))
                return
        except Exception as e:  # a bad picture, a picture host down, no Pillow
            log(f"      {key}: picture: {e}")


# The device's types (app/src/index.rs): what is not of them makes the device leave
# the entry out, so it is cleaned (third-party metadata) or refused (a recipe's).
TEXT, TEXTS = str, "texts"
META_TYPES = {"name": TEXT, "categories": TEXTS, "status": TEXT, "assets": TEXT, "porter": TEXTS,
              "notes": TEXT, "image": TEXT, "url": TEXT, "group": TEXT, "about": TEXT}


def typed(value, kind):
    if kind == TEXTS:
        return isinstance(value, list) and all(isinstance(v, str) for v in value)
    return isinstance(value, kind)


def clean_meta(meta):
    """Catalog metadata (Producdevity's ports.json, not reviewed here) of the types the
    device reads; a field of another type is dropped, not the port."""
    return {k: v for k, v in meta.items() if k not in META_TYPES or typed(v, META_TYPES[k])}


def check_port(p):
    """Raise when an index entry would not be read by the device."""
    def need(cond, what):
        if not cond:
            raise ValueError(f"index entry: {what}")
    for k in ("id", "name", "version", "date", "strip", "into"):
        need(isinstance(p.get(k, ""), str), f"`{k}` is text")
    for k in ("take", "skip", "keep", "requires"):
        need(typed(p.get(k, []), TEXTS), f"`{k}` is a list of texts")
    need(all(isinstance(m, list) and len(m) == 2 and all(isinstance(x, str) for x in m) for m in p.get("map", [])),
         "`map` is [from, to] pairs of texts")
    for n in p.get("needs", []):
        need(isinstance(n, dict) and isinstance(n.get("what"), str), "each need has `what`")
        for k in ("hint", "sha1", "title"):
            need(isinstance(n.get(k, ""), str), f"need `{k}` is text")
        for k in ("check", "search"):
            need(typed(n.get(k, []), TEXTS), f"need `{k}` is a list of texts")
        need(isinstance(n.get("n64", False), bool), "need `n64` is true or false")
    sc = p.get("shortcut")
    need(sc is None or (isinstance(sc, dict) and all(isinstance(v, str) for v in sc.values())), "`shortcut` texts")
    for k, kind in META_TYPES.items():
        need(k not in p or typed(p[k], kind), f"`{k}` type")


# Ports of MiyooMini-Ports without a recipe, tried as their release lays itself out.
AUTO_FILE = "auto (MiyooMini-Ports)"
AUTO_MAX = 600 << 20
# Release files for other systems or not for playing.
AUTO_NOT = re.compile(r"source|src|debug|symbols|spruce|minui|nextui|allium|knulli|muos|trimui|rg35|anbernic"
                      r"|garlic|batocera|windows|win32|win64|linux|macos|android|apk|deb\b|\.ipk", re.I)
AUTO_LIKE = (("onion", 4), ("miyoo", 3), ("mini", 1), ("mmp", 1), ("arm", 1))
# Something that starts: an app's entry or launcher, a shortcut for Ports.
AUTO_STARTS = re.compile(r"^(App/[^/]+/(launch\.sh|config\.json)|Roms/PORTS/Shortcuts/.+\.(port|sh)|Roms/PORTS/[^/]+\.sh)$")


def github_repo(url):
    """owner/repo of a github.com address, or None."""
    m = re.match(r"^https?://(?:www\.)?github\.com/([\w.-]+)/([\w.-]+?)(?:\.git)?(?:[/?#].*)?$", url or "")
    return f"{m.group(1)}/{m.group(2)}" if m else None


def auto_pick(releases):
    """The newest release with one release file that is plainly for OnionOS: archives
    only, none for other systems; of several, the one whose name says Onion or Miyoo
    most, when one does more than the others."""
    for rel in sorted(releases, key=lambda r: r.get("published_at") or "", reverse=True):
        if rel.get("draft") or rel.get("prerelease"):
            continue
        files = [a for a in rel.get("assets", []) if kind_of(a["name"]) and not AUTO_NOT.search(a["name"])]
        if not files:
            continue
        # The same file packed twice (game.zip, game.7z) is one: zip first.
        stem = lambda a: a["name"][: -len(next(e for e in (".tar.gz", ".tgz", ".zip", ".tar", ".7z")
                                               if a["name"].lower().endswith(e)))].lower()
        packing = {"zip": 0, "tar.gz": 1, "tar": 2, "7z": 3}
        files = [a for a in sorted(files, key=lambda a: packing[kind_of(a["name"])])
                 if a is next(b for b in sorted(files, key=lambda b: packing[kind_of(b["name"])]) if stem(b) == stem(a))]
        score = lambda a: sum(n for word, n in AUTO_LIKE if word in a["name"].lower())
        files.sort(key=score, reverse=True)
        if len(files) > 1 and score(files[0]) == score(files[1]):
            raise ValueError(f"{rel.get('tag_name')}: no one file of {', '.join(a['name'] for a in files[:4])} is plainly the one")
        return rel, files[0]
    raise ValueError("no release with an archive for OnionOS")


def auto_entries(catalog, taken_ids, taken_names, gh, cache, site, previous, previous_source):
    """Entries for the ports of MiyooMini-Ports no recipe covers, whose GitHub release
    goes onto the card as it is (App/..., Roms/PORTS/...) with something that starts:
    built and checked like a recipe's, marked `auto`. The rest stay under `manual`."""
    out = []
    for name, c in sorted(catalog.items(), key=lambda kv: str(kv[0]).lower()):
        repo = github_repo(c.get("upstream")) if isinstance(c.get("upstream"), str) else None
        rid = slug(name) if isinstance(name, str) else ""
        if name in taken_names or not repo or not rid or rid in taken_ids:
            continue
        recipe = {"id": rid, "_file": AUTO_FILE, "catalog": name, "github": repo, "asset": ".", "_auto": True}
        try:
            rel, asset = auto_pick(gh.releases(repo))
            if (asset.get("size") or 0) > AUTO_MAX:
                raise ValueError(f"{asset['name']}: larger than {AUTO_MAX >> 20} MB")
            recipe["_release"] = (rel, asset, [])
            try:
                p = entry(recipe, catalog, gh, cache, site)
            except ValueError as e:
                # An app zipped without its folder (launch.sh at the top): into App/<repo>/.
                if not str(e).startswith("nothing under") or not re.search(r"'(launch\.sh|config\.json)'", str(e)):
                    raise
                recipe["into"] = f"App/{re.sub(r'[^A-Za-z0-9._-]', '', repo.split('/')[1])}/"
                p = entry(recipe, catalog, gh, cache, site)
            if not any(AUTO_STARTS.match(f) for f in planned(p)):
                raise ValueError("nothing in it starts (no App/<Name>/launch.sh, no Ports shortcut)")
            p["auto"] = True
            out.append(p)
            taken_ids.add(rid)
            log(f"auto  {rid:24} {p['version']:20} {p['install']['files']:4} files")
        except ValueError as e:
            log(f"      {rid}: not automatic: {e}")
        except Exception as e:  # noqa: BLE001  (a host down: what was there before stays)
            old = previous.get(rid)
            if old and old.get("auto"):
                out.append(old)
                taken_ids.add(rid)
                keep_pictures(old, previous_source, site)
            log(f"      {rid}: not automatic now: {type(e).__name__}: {e}" + (" (kept)" if old else ""))
    return out


def planned(p):
    """The card paths an entry installs, as the catalog lists them: its roots and
    marks are not all of them, so the archive's own list is kept on the entry."""
    return p.pop("_files", [])


def mixtape_release(recipe):
    """A Mixtape catalog's entry as a release: its version, the day it was first seen,
    and its file (checked against the catalog's SHA-256 and size when it gives them)."""
    m = recipe["_mixtape"]
    name = urllib.parse.unquote(urllib.parse.urlsplit(m["url"]).path.rsplit("/", 1)[-1])
    asset = {"name": name, "browser_download_url": m["url"], "size": m.get("size"),
             **({"digest": "sha256:" + m["sha256"].lower()} if m.get("sha256") else {})}
    return {"tag_name": m["version"], "published_at": recipe["_date"]}, asset, []


def entry(recipe, catalog, gh, cache, site, warnings=None):
    warnings = [] if warnings is None else warnings
    if "_mixtape" in recipe:
        meta = clean_meta({k: v for k, v in recipe["_mixtape"].items() if k in META_TYPES})
        meta["url"] = recipe["_mixtape"].get("upstream") if isinstance(recipe["_mixtape"].get("upstream"), str) else ""
    else:
        meta = clean_meta(dict(catalog.get(recipe.get("catalog"), {})))
        meta["url"] = meta.pop("upstream", None) or f"https://github.com/{recipe['github']}"
    for k in META + ("group", "about"):
        if k in recipe:
            meta[k] = recipe[k]
    if "name" not in meta:
        raise ValueError("no name (no catalog entry and none in the recipe)")
    if "_release" in recipe:
        rel, asset, passed = recipe["_release"]
    elif "_mixtape" in recipe:
        rel, asset, passed = mixtape_release(recipe)
    else:
        rel, asset, passed = pick_release(gh.releases(recipe["github"]), recipe)
    if not asset:
        raise ValueError("no release with a matching file")
    if passed:
        warnings.append({"id": recipe["id"], "file": recipe.get("_file", ""),
                         "warning": f"newer release {', '.join(passed)} has no file matching `asset`: "
                                    f"{rel['tag_name']} is used"})
    if not kind_of(asset["name"]):
        raise ValueError(f"cannot unpack {asset['name']}")
    path = download(asset, cache)
    digest = sha256(path)
    # GitHub gives a digest for files uploaded since mid-2025 only (null before). A
    # mismatch may be a stale cache: downloaded once more before giving up.
    if asset.get("digest") and asset["digest"] != "sha256:" + digest:
        path = download(asset, cache, fresh=True)
        digest = sha256(path)
        if asset["digest"] != "sha256:" + digest:
            raise ValueError(f"downloaded file does not match GitHub's digest {asset['digest']}")
    kind = sniff(path) or kind_of(asset["name"])
    strip, files, size, skipped, origin = plan(entries(path, kind), recipe)
    checks = shortcut_checks(path, kind, origin, files)
    if recipe.get("requires"):
        # the shortcut of a game on a runtime checks the runtime's files: those come with it
        own = roots(files, True)
        checks = [c for c in checks if any(c.startswith(r) for r in own)]
    needs = needs_of(recipe, checks)
    out = {
        "id": recipe["id"],
        **{k: meta[k] for k in META + ("group", "about") if meta.get(k) not in (None, "", [])},
        "version": rel["tag_name"],
        "date": (rel.get("published_at") or "")[:10],
        "file": {
            "url": asset["browser_download_url"],
            "name": asset["name"],
            "size": path.stat().st_size,
            "sha256": digest,
            "kind": kind,
        },
        "strip": strip,
        "take": recipe.get("take", DEFAULT_TAKE),
        **{k: recipe[k] for k in ("into", "map", "skip", "shortcut") if recipe.get(k)},
        "keep": recipe.get("keep", []),
        "needs": needs,
        **({"requires": recipe["requires"]} if recipe.get("requires") else {}),
        "install": {"files": len(files), "bytes": size, "roots": roots(files, bool(recipe.get("requires"))),
                    "marks": marks(files, recipe.get("keep", []), bool(recipe.get("requires")))},
        **({"source": recipe["_source"]} if "_source" in recipe else {}),
    }
    if skipped:
        out["skipped"] = skipped
    if recipe.get("_auto"):
        out["_files"] = files
    from_archive = lambda: archive_image(path, kind, origin, files)
    from_catalog = lambda: meta.get("image") and fetch(meta["image"], cache)
    add_pictures(out, recipe["id"], [from_archive, from_catalog], site)
    check_port(out)
    return out


def keep_pictures(item, previous_source, site):
    """A port kept from the previous index keeps its pictures: copied over from it."""
    base = previous_source.rsplit("/", 1)[0] if previous_source else ""
    for kind in SIZES:
        name = item.get(kind)
        if not name or (site / name).exists():
            continue
        try:
            if re.match(r"^https?://", base):
                req = urllib.request.Request(f"{base}/{name}", headers={"User-Agent": "miyoo-ports-index"})
                with urllib.request.urlopen(req, timeout=60) as r:
                    data = r.read()
            else:
                data = (Path(base or ".") / name).read_bytes()
            (site / name).parent.mkdir(parents=True, exist_ok=True)
            (site / name).write_bytes(data)
        except (OSError, urllib.error.URLError) as e:
            log(f"      {item['id']}: picture {name}: {e}")
            item.pop(kind)


def slug(name):
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", help="the index to write (default site/index.json; needed with --only)")
    ap.add_argument("--cache", default=os.environ.get("PORTS_CACHE", ".cache"))
    ap.add_argument("--catalog", default=CATALOG_URL, help="ports.json, URL or file")
    ap.add_argument("--previous", help="the published index (URL or file): kept for ports that fail now")
    ap.add_argument("--only", nargs="*", help="recipe ids to build (the rest are left out)")
    ap.add_argument("--no-auto", action="store_true", help="no automatic entries for ports without a recipe")
    args = ap.parse_args()
    if args.out is None:
        if args.only:
            raise SystemExit("--only builds a part of the catalog: give --out, so site/index.json stays whole")
        args.out = "site/index.json"

    cache = Path(args.cache)
    cache.mkdir(parents=True, exist_ok=True)
    try:
        catalog = {p["name"]: p for p in (load_json(args.catalog) or {}).get("ports", [])
               if isinstance(p, dict) and isinstance(p.get("name"), str)}
    except (OSError, ValueError, urllib.error.URLError) as e:
        raise SystemExit(f"catalog {args.catalog}: {e}")
    previous_index = {}
    for attempt in range(3):
        try:
            previous_index = load_json(args.previous) or {}
            break
        except urllib.error.HTTPError as e:
            if e.code == 404:  # nothing published yet
                break
            error = e
        except (OSError, ValueError, urllib.error.URLError) as e:
            error = e
        if attempt == 2:
            # Without it, ports failing now would drop out and "failing since" restart.
            raise SystemExit(f"previous index {args.previous}: {error}")
    previous = {p["id"]: p for p in previous_index.get("ports", []) + previous_index.get("dev", [])}
    # A recipe failing since an earlier build (not just now: a host down for a minute).
    previous_failed = {f["id"]: f for f in previous_index.get("failed", [])}
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    gh = Github(os.environ.get("GITHUB_TOKEN"))
    site = Path(args.out).parent
    site.mkdir(parents=True, exist_ok=True)

    ports, dev, failed, warnings = [], [], [], []
    recipes = load_recipes()
    sourced = source_recipes(previous=previous)
    # One id, one port: a source's port of a recipe's id (or another source's) fails.
    taken = {}
    for r in recipes + sourced:
        taken.setdefault(r["id"], []).append(r["_file"])
    for r in sourced:
        others = [f for f in taken[r["id"]] if f != r["_file"]]
        if others and "_error" not in r:
            r["_error"] = f"id {r['id']} is also the id of {', '.join(others)}"
    recipes = [r for r in recipes + sourced if not args.only or r["id"] in args.only]
    for recipe in recipes:
        shelf = dev if recipe.get("_dev") else ports
        try:
            if "_error" in recipe:
                raise ValueError(recipe["_error"])
            p = entry(recipe, catalog, gh, cache, site, warnings)
            log(f"ok    {p['id']:24} {p['version']:20} {p['install']['files']:4} files"
                + (f"  skipped {p['skipped']}" if p.get("skipped") else ""))
            shelf.append(p)
        # Whatever one release or recipe throws (a cut-off download, a broken 7z, an
        # odd zip method, a wrong field type) fails that recipe, not the build.
        except Exception as e:  # noqa: BLE001
            e = f"{type(e).__name__}: {e}" if not isinstance(e, ValueError) else e
            since = previous_failed.get(recipe["id"], {}).get("since", now)
            failed.append({"id": recipe["id"], "file": recipe["_file"], "error": str(e), "since": since})
            # A source that could not be read keeps all its ports of before.
            kept = [previous[recipe["id"]]] if recipe["id"] in previous else []
            if recipe.get("_whole"):
                kept = [p for p in previous_index.get("dev" if recipe["_dev"] else "ports", [])
                        if p.get("source") == recipe["_source"]]
            for old in kept:
                shelf.append(old)
                keep_pictures(old, args.previous, site)
            log(f"FAIL  {recipe['id']:24} {e}" + (f" (kept {', '.join(o['id'] + ' ' + o['version'] for o in kept)})" if kept else ""))

    if not args.only and not args.no_auto:
        names = {r.get("catalog") for r in load_recipes()} | {p["name"] for p in ports + dev}
        ids = {r["id"] for r in recipes} | {p["id"] for p in ports + dev}
        ports += auto_entries(catalog, ids, names, gh, cache, site, previous, args.previous)
    # a port that runs on another the index does not have cannot be installed: said so
    have = {p["id"] for p in ports + dev}
    for p in ports + dev:
        for req in p.get("requires", []):
            if req not in have:
                warnings.append({"id": p["id"], "file": "", "warning": f"requires {req}, which is not in the index"})
    covered = {r.get("catalog") for r in recipes} | {p["name"] for p in ports}
    manual = [
        clean_meta({k: c[k] for k in ("name", "categories", "status", "assets", "notes", "porter", "image")
                    if c.get(k)})
        | ({"url": c["upstream"]} if isinstance(c.get("upstream"), str) else {})
        for name, c in sorted(catalog.items(), key=lambda kv: str(kv[0]).lower())
        if name not in covered and isinstance(name, str)
    ]
    for m in manual:
        add_pictures(m, "m-" + slug(m["name"]), [lambda: m.get("image") and fetch(m["image"], cache)], site)
    index = {
        "format": FORMAT,
        "generated": now,
        "ports": sorted(ports, key=lambda p: p["name"].lower()),
        # Development builds apart: Port Store before 1.1 does not read this list.
        "dev": sorted(dev, key=lambda p: p["name"].lower()),
        "manual": manual,
        "failed": failed,
        "warnings": warnings,
    }
    out = Path(args.out)
    if recipes and len(failed) == len(recipes):
        log(f"every recipe failed: {out} left as it was")
        return 1
    out.write_text(json.dumps(index, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    # Pictures no entry names any more (from an earlier build in the same folder) go.
    named = {item.get(k) for item in index["ports"] + dev + manual for k in SIZES}
    for old in (site / "img").glob("*.png"):
        if f"img/{old.name}" not in named:
            old.unlink()
    if not args.only:
        for old in cache.iterdir():
            if old.is_file() and old.name not in USED:
                old.unlink()
    for w in warnings:
        log(f"WARN  {w['id']:24} {w['warning']}")
    log(f"{len(ports)} ports, {len(dev)} dev, {len(manual)} manual, {len(failed)} failed, {len(warnings)} warnings -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
