#!/usr/bin/env python3
"""Minimal git command surface used by tools.gitlibs over a clj-nix cache."""

import json
import os
import sys
from pathlib import Path


def fail(message: str, status: int = 2) -> "None":
    print(f"clj-lock git shim: {message}", file=sys.stderr)
    raise SystemExit(status)


def parse_args(argv: list[str]) -> tuple[Path, list[str]]:
    git_dir: str | None = None
    args: list[str] = []
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--git-dir":
            index += 1
            if index == len(argv):
                fail("--git-dir requires a value")
            git_dir = argv[index]
        elif arg.startswith("--git-dir="):
            git_dir = arg.split("=", 1)[1]
        elif arg == "-C":
            index += 1
            if index == len(argv):
                fail("-C requires a directory")
            os.chdir(argv[index])
        else:
            args.append(arg)
        index += 1
    if git_dir is None:
        git_dir = ".git"
    return Path(git_dir), args


def revision_data(git_dir: Path) -> list[dict]:
    revs = git_dir / "revs"
    if not revs.is_dir():
        fail(f"{revs} does not exist")
    records = []
    for path in sorted(revs.iterdir()):
        if path.is_file():
            records.append(json.loads(path.read_text()))
    return records


def resolve_commit(records: list[dict], token: str) -> str | None:
    token = token.removesuffix("^{commit}")
    for record in records:
        tag = record.get("tag")
        revision = record.get("rev", "")
        if tag == token or revision.startswith(token):
            return revision
    return None


def main() -> None:
    git_dir, args = parse_args(sys.argv[1:])
    records = revision_data(git_dir)
    if not args:
        fail("no command supplied")

    command, *rest = args
    if command == "fetch":
        return
    if command == "tag" and rest == ["--sort=v:refname"]:
        for tag in sorted(filter(None, (record.get("tag") for record in records))):
            print(tag)
        return
    if command == "rev-parse" and len(rest) == 1:
        revision = resolve_commit(records, rest[0])
        if revision is None:
            raise SystemExit(1)
        print(revision)
        return
    if command == "merge-base" and len(rest) == 3 and rest[0] == "--is-ancestor":
        # Match clj-nix's fake_git.clj exactly. The lock records ancestry on
        # the first revision's metadata using the second revision as the key.
        revision = resolve_commit(records, rest[1]) or rest[1]
        ancestor = resolve_commit(records, rest[2]) or rest[2]
        record = next((record for record in records if record.get("rev") == revision), None)
        if record is None:
            raise SystemExit(1)
        if record.get("ancestor?", {}).get(ancestor, False):
            return
        raise SystemExit(1)

    fail(f"unsupported command: {' '.join(args)}")


if __name__ == "__main__":
    main()
