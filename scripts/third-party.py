#!/usr/bin/env python3
"""Write data/third-party.txt: every Rust crate built into Depot, with its licence.

Run it after changing dependencies:  ./scripts/third-party.py
The About window shows the file under Legal, as the crates' licences ask.
"""
import json
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parent.parent
meta = json.loads(
    subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", "x86_64-unknown-linux-gnu"],
        cwd=root,
    )
)
ours = set(meta["workspace_members"])
# only crates that end up in the program: normal dependencies, not build tools or test helpers
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
used, todo = set(), list(ours)
while todo:
    for d in nodes[todo.pop()]["deps"]:
        if any(k["kind"] in (None, "normal") for k in d["dep_kinds"]) and d["pkg"] not in used:
            used.add(d["pkg"])
            todo.append(d["pkg"])
pkgs = sorted((p for p in meta["packages"] if p["id"] in used - ours), key=lambda p: p["name"])
lines = [f"{p['name']} {p['version']}: {p.get('license') or 'see its repository'}" for p in pkgs]
(root / "data/third-party.txt").write_text("\n".join(lines) + "\n")
print(f"{len(lines)} crates")
