#!/usr/bin/env python3
"""Check recipes as the catalog build would take them, and say what each gives: for a
pull request, so a recipe can be judged without building it by hand.

    python3 index/check_recipes.py recipes/ports/foo.json [...]   # markdown on stdout

Each recipe is checked for its fields, then built like index/build.py does (its
release found and downloaded, its layout planned, its pictures made). A source
(recipes/sources/*.json, a porter's catalog in Mixtape's format) is read and each of
its ports built the same way. Exit status 1 when one fails.
"""
import json
import re
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build  # noqa: E402  (index/build.py, next to this file)

FIELDS = {
    "id", "catalog", "name", "group", "categories", "status", "assets", "porter", "about", "url",
    "image", "notes", "github", "asset", "tag", "prerelease", "strip", "take", "into", "map", "skip",
    "keep", "needs", "shortcut",
}
NEED_FIELDS = {"what", "hint", "check", "sha1", "search", "n64", "title"}


TEXT_FIELDS = {"id", "catalog", "name", "group", "status", "assets", "about", "url", "image", "notes",
               "github", "asset", "tag", "strip", "into"}
LIST_FIELDS = {"categories", "porter", "take", "skip", "keep"}


def card_path(path):
    """As the device's index::card_path: relative, never climbing out."""
    return not path.startswith("/") and "\\" not in path and "\0" not in path \
        and not any(part in ("..", ".") for part in path.split("/"))


def problems(recipe, path, ids):
    out = []
    for k in TEXT_FIELDS & set(recipe):
        if not isinstance(recipe[k], str):
            out.append(f"`{k}` is text")
    for k in LIST_FIELDS & set(recipe):
        if not (isinstance(recipe[k], list) and all(isinstance(v, str) for v in recipe[k])):
            out.append(f"`{k}` is a list of texts")
    if "prerelease" in recipe and not isinstance(recipe["prerelease"], bool):
        out.append("`prerelease` is true or false")
    paths = [recipe.get("into", "")] + [v for k in ("take", "skip", "keep") for v in recipe.get(k, []) if isinstance(v, str)]
    paths += [m[1] for m in recipe.get("map", []) if isinstance(m, list) and len(m) == 2 and isinstance(m[1], str)]
    for need in recipe.get("needs", []) if isinstance(recipe.get("needs", []), list) else []:
        if isinstance(need, dict):
            paths += [v for k in ("check", "search") for v in need.get(k, []) if isinstance(v, str)]
    if isinstance(recipe.get("shortcut"), dict):
        paths += [v for v in recipe["shortcut"].values() if isinstance(v, str)]
    for bad in [p for p in paths if isinstance(p, str) and not card_path(p)]:
        out.append(f"`{bad}`: paths are relative to the card, without `..`")
    unknown = set(recipe) - FIELDS
    if unknown:
        out.append(f"unknown fields: {', '.join(sorted(unknown))}")
    rid = recipe.get("id", "")
    if not isinstance(rid, str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]*", rid):
        out.append("`id`: lower-case letters, digits and dashes")
    else:
        others = [p for p in ids.get(rid, []) if Path(p).resolve() != Path(path).resolve()]
        if others:
            out.append(f"`id` {rid} is taken by {', '.join(others)}")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", str(recipe.get("github", ""))):
        out.append("`github`: owner/repository")
    for key in ("asset", "tag"):
        try:
            re.compile(str(recipe.get(key, "")))
        except re.error as e:
            out.append(f"`{key}` is not a regular expression: {e}")
    if "asset" not in recipe:
        out.append("`asset`: the release file's name, as a regular expression")
    if "catalog" not in recipe and "name" not in recipe:
        out.append("`catalog` (the name in MiyooMini-Ports) or `name`")
    for need in recipe.get("needs", []) if isinstance(recipe.get("needs", []), list) else [None]:
        if not isinstance(need, dict):
            out.append("each of `needs` is an object")
            continue
        if set(need) - NEED_FIELDS:
            out.append(f"unknown fields in `needs`: {', '.join(sorted(set(need) - NEED_FIELDS))}")
        if not need.get("what"):
            out.append("each of `needs` says `what`")
    for pair in recipe.get("map", []):
        if not (isinstance(pair, list) and len(pair) == 2 and all(isinstance(x, str) for x in pair)):
            out.append("`map`: [from, to] pairs of texts")
    return out


def report(entry):
    lines = [
        f"- release **{entry['version']}** ({entry.get('date', '?')}): `{entry['file']['name']}`, "
        f"{entry['file']['size'] / 1048576:.1f} MB, {entry['file']['kind']}",
        f"- onto the card: {entry['install']['files']} files, {entry['install']['bytes'] / 1048576:.1f} MB, "
        f"in {', '.join(f'`{r}`' for r in entry['install']['roots']) or '(no folder of its own)'}",
    ]
    if entry.get("skipped"):
        lines.append(f"- left out: {', '.join(f'`{s}`' for s in entry['skipped'][:12])}")
    games = "Apps" if "app" in entry.get("categories", []) else "Games → MiyooPorts"
    lines.append(f"- shown in **{games}**, categories {', '.join(entry.get('categories', [])) or '(none)'}")
    for need in entry.get("needs", []):
        how = ", ".join(f"`{c}`" for c in need.get("check", [])) or ("ROM by SHA-1" if need.get("sha1") else "told only")
        lines.append(f"- the user brings: {need['what']} ({how})")
    lines.append(f"- pictures: {'yes' if entry.get('thumb') else '**none**'}")
    return "\n".join(lines)


def check_source(path, ids, catalog, gh, cache, site, out):
    """A source's ports, each built: how many fail."""
    failed = 0
    for recipe in build.source_recipes([path]):
        name = recipe.get("name", recipe["id"])
        taken = [f for f in ids.get(recipe["id"], []) if Path(f).resolve() != Path(path).resolve()]
        if "_error" not in recipe and taken:
            recipe["_error"] = f"id {recipe['id']} is taken by {', '.join(taken)}"
        if "_error" in recipe:
            out.append(f"❌ {name}: {recipe['_error']}")
            failed += 1
            continue
        try:
            warnings = []
            entry = build.entry(recipe, catalog, gh, cache, site, warnings)
            dev = " (development build)" if recipe.get("_dev") else ""
            out += [f"✅ **{entry['name']}**{dev}, id `{entry['id']}`", report(entry)]
            out += [f"- ⚠️ {w['warning']}" for w in warnings]
        except Exception as e:  # what the catalog build would stop at, said plainly
            out.append(f"❌ {name}: the build stops here: {e}")
            failed += 1
    out.append("")
    return failed


def main(paths):
    all_ids = {}
    for p in sorted(Path("recipes").rglob("*.json")):
        if p.parent.name == "sources":
            continue
        try:
            all_ids.setdefault(json.loads(p.read_text(encoding="utf-8")).get("id"), []).append(str(p))
        except (ValueError, AttributeError):
            pass
    catalog = {p["name"]: p for p in (build.load_json(build.CATALOG_URL) or {}).get("ports", [])}
    gh = build.Github(build.os.environ.get("GITHUB_TOKEN"))
    out, failed = ["## Recipe check", ""], 0
    with tempfile.TemporaryDirectory() as tmp:
        cache, site = Path(tmp) / "cache", Path(tmp) / "site"
        cache.mkdir()
        site.mkdir()
        for path in paths:
            out.append(f"### `{path}`")
            if Path(path).parent.name == "sources":
                failed += check_source(path, all_ids, catalog, gh, cache, site, out)
                continue
            try:
                recipe = json.loads(Path(path).read_text(encoding="utf-8"))
            except (OSError, ValueError) as e:
                out += [f"❌ not readable JSON: {e}", ""]
                failed += 1
                continue
            bad = problems(recipe, path, all_ids)
            if recipe.get("catalog") and recipe["catalog"] not in catalog:
                bad.append(f"`catalog`: no entry named {recipe['catalog']!r} in MiyooMini-Ports")
            if bad:
                out += [f"❌ {b}" for b in bad] + [""]
                failed += 1
                continue
            try:
                warnings = []
                entry = build.entry(recipe, catalog, gh, cache, site, warnings)
                out += [f"✅ **{entry['name']}**", report(entry)]
                out += [f"- ⚠️ {w['warning']}" for w in warnings] + [""]
            except Exception as e:  # what the catalog build would stop at, said plainly
                out += [f"❌ the build stops here: {e}", ""]
                failed += 1
    out.append(f"{len(paths) - failed} of {len(paths)} recipes build." if paths else "No recipes changed.")
    print("\n".join(out))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
