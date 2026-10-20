#!/usr/bin/env python3
"""Keep one GitHub issue up to date with the recipes that fail: opened (or updated)
while any has failed since an earlier build, closed when none does. In CI only.
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


index = json.load(open(sys.argv[1], encoding="utf-8"))
lasting = [f for f in index.get("failed", []) if f.get("since", index["generated"]) < index["generated"]]
open_issues = [i for i in api("GET", f"/issues?state=open&labels={LABEL}") if i["title"] == TITLE]
if lasting:
    rows = "\n".join(f"| `{f['id']}` | [{f['file']}](../blob/main/{f['file']}) | {f['since'][:10]} | "
                     f"{f['error'].replace('|', '/')[:300]} |" for f in lasting)
    body = ("These recipes failed in the last two builds or more: the catalog keeps their last good "
            "version (or leaves them out). Most often the author renamed the release file or changed "
            "its layout.\n\n| Recipe | File | Failing since | Error |\n|---|---|---|---|\n" + rows +
            f"\n\nCatalog built {index['generated']}. This issue closes by itself once they build again.")
    if open_issues:
        api("PATCH", f"/issues/{open_issues[0]['number']}", {"body": body})
    else:
        api("POST", "/issues", {"title": TITLE, "body": body, "labels": [LABEL]})
    print(f"{len(lasting)} recipes failing")
else:
    for i in open_issues:
        api("POST", f"/issues/{i['number']}/comments", {"body": "All recipes build again."})
        api("PATCH", f"/issues/{i['number']}", {"state": "closed"})
    print("no recipes failing")
