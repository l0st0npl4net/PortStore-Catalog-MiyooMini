#!/usr/bin/env python3
"""Check recipes as the catalog build would take them, and say what each gives: for a
pull request, so a recipe can be judged without building it by hand.

    python3 index/check_recipes.py recipes/ports/foo.json [...]   # markdown on stdout

Each recipe is checked for its fields, then built like index/build.py does (its
release found and downloaded, its layout planned, its pictures made). Exit status 1
when one fails.
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


def problems(recipe, path, ids):
    out = []
    unknown = set(recipe) - FIELDS
    if unknown:
        out.append(f"unknown fields: {', '.join(sorted(unknown))}")
    rid = recipe.get("id", "")
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", rid):
        out.append("`id`: lower-case letters, digits and dashes")
    elif ids.get(rid, path) != path:
        out.append(f"`id` {rid} is taken by {ids[rid]}")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", recipe.get("github", "")):
        out.append("`github`: owner/repository")
    for key in ("asset", "tag"):
        try:
            re.compile(recipe.get(key, ""))
        except re.error as e:
            out.append(f"`{key}` is not a regular expression: {e}")
    if "asset" not in recipe:
        out.append("`asset`: the release file's name, as a regular expression")
    if "catalog" not in recipe and "name" not in recipe:
        out.append("`catalog` (the name in MiyooMini-Ports) or `name`")
    for need in recipe.get("needs", []):
        if set(need) - NEED_FIELDS:
            out.append(f"unknown fields in `needs`: {', '.join(sorted(set(need) - NEED_FIELDS))}")
        if not need.get("what"):
            out.append("each of `needs` says `what`")
    for pair in recipe.get("map", []):
        if not (isinstance(pair, list) and len(pair) == 2):
            out.append("`map`: [from, to] pairs")
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


def main(paths):
    all_ids = {}
    for p in sorted(Path("recipes").rglob("*.json")):
        try:
            all_ids.setdefault(json.loads(p.read_text(encoding="utf-8")).get("id"), str(p))
        except ValueError:
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
                entry = build.entry(recipe, catalog, gh, cache, site)
                out += [f"✅ **{entry['name']}**", report(entry), ""]
            except Exception as e:  # what the catalog build would stop at, said plainly
                out += [f"❌ the build stops here: {e}", ""]
                failed += 1
    out.append(f"{len(paths) - failed} of {len(paths)} recipes build." if paths else "No recipes changed.")
    print("\n".join(out))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
