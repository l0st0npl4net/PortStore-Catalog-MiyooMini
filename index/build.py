#!/usr/bin/env python3
"""Build index.json, the catalog Port Store reads on the device.

For each recipe (recipes/**/*.json) it finds the newest release of the port's GitHub
repository with a matching file, downloads that file, checks what is inside and which
part of it goes onto the SD card, and writes the file's address, size and SHA-256 with
the install rules. Catalog entries without a recipe (Producdevity/MiyooMini-Ports
ports.json) are listed as "manual": a link only.

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


def roots(files):
    """The folders the port owns, App/<Name>/ (Emu/, RApp/) and Roms/PORTS/Games/<Name>/: how the
    device tells a port put on the card by hand."""
    out = set()
    for f in files:
        p = f.split("/")
        if p[0] in ("App", "Emu", "RApp") and len(p) > 2:
            out.add(f"{p[0]}/{p[1]}/")
        elif p[:3] == ["Roms", "PORTS", "Games"] and len(p) > 4:
            out.add("/".join(p[:4]) + "/")
    return sorted(out)


def marks(files, keep):
    """A file of the port's in each of its folders (not one the user keeps): all on
    the card, the port is there."""
    kept = lambda f: any(f.startswith(k) if k.endswith("/") else f == k for k in keep)
    out = []
    for root in roots(files):
        mine = sorted(f for f in files if f.startswith(root) and not kept(f))
        if mine:
            out.append(min(mine, key=lambda f: (f.count("/"), f)))
    return out


def load_recipes():
    """Every recipe, with its file; a recipe that cannot be read, or whose id another
    recipe has too, comes with the reason it fails (`_error`), the rest still build."""
    out = []
    for path in sorted((ROOT / "recipes").rglob("*.json")):
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
    for k in ("take", "skip", "keep"):
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


def entry(recipe, catalog, gh, cache, site, warnings=None):
    warnings = [] if warnings is None else warnings
    meta = clean_meta(dict(catalog.get(recipe.get("catalog"), {})))
    meta["url"] = meta.pop("upstream", None) or f"https://github.com/{recipe['github']}"
    for k in META + ("group", "about"):
        if k in recipe:
            meta[k] = recipe[k]
    if "name" not in meta:
        raise ValueError("no name (no catalog entry and none in the recipe)")
    rel, asset, passed = pick_release(gh.releases(recipe["github"]), recipe)
    if not asset:
        raise ValueError("no release with a matching file")
    if passed:
        warnings.append({"id": recipe["id"], "file": recipe["_file"],
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
    needs = needs_of(recipe, shortcut_checks(path, kind, origin, files))
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
        "install": {"files": len(files), "bytes": size, "roots": roots(files),
                    "marks": marks(files, recipe.get("keep", []))},
    }
    if skipped:
        out["skipped"] = skipped
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
    previous = {p["id"]: p for p in previous_index.get("ports", [])}
    # A recipe failing since an earlier build (not just now: a host down for a minute).
    previous_failed = {f["id"]: f for f in previous_index.get("failed", [])}
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    gh = Github(os.environ.get("GITHUB_TOKEN"))
    site = Path(args.out).parent
    site.mkdir(parents=True, exist_ok=True)

    ports, failed, warnings = [], [], []
    recipes = [r for r in load_recipes() if not args.only or r["id"] in args.only]
    for recipe in recipes:
        try:
            if "_error" in recipe:
                raise ValueError(recipe["_error"])
            p = entry(recipe, catalog, gh, cache, site, warnings)
            log(f"ok    {p['id']:24} {p['version']:20} {p['install']['files']:4} files"
                + (f"  skipped {p['skipped']}" if p.get("skipped") else ""))
            ports.append(p)
        # Whatever one release or recipe throws (a cut-off download, a broken 7z, an
        # odd zip method, a wrong field type) fails that recipe, not the build.
        except Exception as e:  # noqa: BLE001
            e = f"{type(e).__name__}: {e}" if not isinstance(e, ValueError) else e
            since = previous_failed.get(recipe["id"], {}).get("since", now)
            failed.append({"id": recipe["id"], "file": recipe["_file"], "error": str(e), "since": since})
            if recipe["id"] in previous:
                ports.append(previous[recipe["id"]])
                keep_pictures(previous[recipe["id"]], args.previous, site)
                log(f"FAIL  {recipe['id']:24} {e} (kept {previous[recipe['id']]['version']})")
            else:
                log(f"FAIL  {recipe['id']:24} {e}")

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
    named = {item.get(k) for item in index["ports"] + manual for k in SIZES}
    for old in (site / "img").glob("*.png"):
        if f"img/{old.name}" not in named:
            old.unlink()
    if not args.only:
        for old in cache.iterdir():
            if old.is_file() and old.name not in USED:
                old.unlink()
    for w in warnings:
        log(f"WARN  {w['id']:24} {w['warning']}")
    log(f"{len(ports)} ports, {len(manual)} manual, {len(failed)} failed, {len(warnings)} warnings -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
