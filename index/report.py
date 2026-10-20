#!/usr/bin/env python3
"""Keep one GitHub issue up to date with what goes wrong in the catalog build:
recipes failing since an earlier build (not one failing just now: a host down for a
minute), and warnings (a release published without its file). Opened, or the last
one reopened, while there is any; closed when there is none. In CI only.
    GITHUB_TOKEN=… GITHUB_REPOSITORY=owner/repo python3 index/report.py site/index.json"""
import json
import os
import sys
import urllib.request

TITLE = "Recipes failing"
LABEL = "recipe-failure"
repo, token = os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_TOKEN"]


def api(method, path, body=None):
    req = urllib.request.Request(
        f"https://api.github.com/repos/{repo}{path}", method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json",
                 "User-Agent": "miyoo-ports-index"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r) if r.status != 204 else None


def cell(text):
    return str(text).replace("|", "/").replace("\n", " ")[:300]


index = json.load(open(sys.argv[1], encoding="utf-8"))
failed = index.get("failed", [])
lasting = [f for f in failed if f.get("since", index["generated"]) < index["generated"]]
warnings = index.get("warnings", [])
issues = [i for i in api("GET", f"/issues?state=all&labels={LABEL}&per_page=20") if i["title"] == TITLE]
issue = next((i for i in issues if i["state"] == "open"), issues[0] if issues else None)

if lasting or warnings:
    parts = []
    if lasting:
        parts.append("These recipes failed in the last two builds or more: the catalog keeps their last good "
                     "version (or leaves them out). Most often the author renamed the release file or changed "
                     "its layout.\n\n| Recipe | File | Failing since | Error |\n|---|---|---|---|\n"
                     + "\n".join(f"| `{f['id']}` | [{f['file']}](../blob/main/{f['file']}) | {f['since'][:10]} | "
                                 f"{cell(f['error'])} |" for f in lasting))
    if warnings:
        parts.append("Warnings:\n\n| Recipe | File | Warning |\n|---|---|---|\n"
                     + "\n".join(f"| `{w['id']}` | [{w['file']}](../blob/main/{w['file']}) | {cell(w['warning'])} |"
                                 for w in warnings))
    body = "\n\n".join(parts) + f"\n\nCatalog built {index['generated']}. This issue closes by itself once all is well."
    if issue:
        api("PATCH", f"/issues/{issue['number']}", {"body": body, "state": "open"})
    else:
        api("POST", "/issues", {"title": TITLE, "body": body, "labels": [LABEL]})
    print(f"{len(lasting)} recipes failing, {len(warnings)} warnings")
elif not failed and issue and issue["state"] == "open":
    # Closed only when nothing fails at all: a recipe failing for the first time
    # keeps it open until the next build tells.
    api("POST", f"/issues/{issue['number']}/comments", {"body": "All recipes build again."})
    api("PATCH", f"/issues/{issue['number']}", {"state": "closed"})
    print("closed")
else:
    print("nothing to report")
