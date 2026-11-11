#!/usr/bin/env python3
"""Render release templates and write the staged release manifest.

template IN OUT [--set KEY=VALUE]... [--enable BLOCK]...
    Keeps @IF_BLOCK@...@END_BLOCK@ sections only for enabled blocks, replaces
    @KEY@ placeholders and refuses to leave any placeholder unfilled.

manifest OUT --field KEY=VALUE... --json-field KEY=JSON... --asset NAME=OS:ARCH...
    Writes release.json; asset checksums are computed from the files beside it.
"""

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

PLACEHOLDER = re.compile(r"@[A-Z][A-Z0-9_]*@")
BLOCK = re.compile(r"@IF_([A-Z0-9_]+)@(.*?)@END_\1@", re.DOTALL)


def pairs(values):
    out = {}
    for item in values or []:
        key, sep, value = item.partition("=")
        if not sep or not key:
            sys.exit(f"expected KEY=VALUE, got {item!r}")
        out[key] = value
    return out


def render(args):
    text = Path(args.input).read_text()
    enabled = set(args.enable or [])
    text = BLOCK.sub(lambda m: m.group(2) if m.group(1) in enabled else "", text)
    for key, value in pairs(args.set).items():
        text = text.replace(f"@{key}@", value)
    left = sorted(set(PLACEHOLDER.findall(text)))
    if left:
        sys.exit(f"unfilled placeholders in {args.input}: {', '.join(left)}")
    Path(args.output).write_text(text)


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def manifest(args):
    out = Path(args.output)
    data = {"product": "ClipSync"}
    data.update(pairs(args.field))
    for key, raw in pairs(args.json_field).items():
        data[key] = json.loads(raw)
    data["staged_at"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    assets = []
    for item in args.asset or []:
        name, _, where = item.partition("=")
        os_name, _, arch = where.partition(":")
        path = out.parent / "assets" / name
        if not path.is_file():
            sys.exit(f"asset missing: {path}")
        assets.append({"name": name, "os": os_name, "arch": arch, "sha256": sha256(path)})
    data["assets"] = assets
    out.write_text(json.dumps(data, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    t = sub.add_parser("template")
    t.add_argument("input")
    t.add_argument("output")
    t.add_argument("--set", action="append")
    t.add_argument("--enable", action="append")
    m = sub.add_parser("manifest")
    m.add_argument("output")
    m.add_argument("--field", action="append")
    m.add_argument("--json-field", action="append")
    m.add_argument("--asset", action="append")
    args = parser.parse_args()
    render(args) if args.command == "template" else manifest(args)


if __name__ == "__main__":
    main()
