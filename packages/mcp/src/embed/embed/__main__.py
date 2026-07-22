"""The embed battery as a CLI: ``python -m embed`` / ``nix run .#embed``.

One subcommand per public function (``dupes`` / ``pairs`` / ``similar`` /
``ensure``), so Elixir and shell callers reach the duplicate-code finder
without a Python kernel (index#3905). Human-readable polars tables by default;
``--json`` emits one row-oriented JSON document. An :class:`embed.EmbedError`
prints to stderr and exits 1.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import polars as pl

import embed


def _emit(frame: pl.DataFrame, *, as_json: bool) -> None:
    """Render ``frame``: one JSON document, or the full (untruncated) table."""
    if as_json:
        sys.stdout.write(frame.write_json() + "\n")
        return
    with pl.Config(tbl_rows=frame.height, fmt_str_lengths=120):
        print(frame)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="embed",
        description="Embedding-based duplicate-code finder and semantic code search "
        "over the ix-mcp parquet cache.",
    )
    commands = parser.add_subparsers(dest="command", required=True)

    dupes = commands.add_parser("dupes", help="top duplicate function pairs under a root")
    dupes.add_argument("root", nargs="?", default=".", help="repo root to chunk and mine (default: .)")
    dupes.add_argument("--k", type=int, default=20, help="pairs to report (default: 20)")

    pairs = commands.add_parser("pairs", help="top pairs across the whole cache (every repo ever embedded)")
    pairs.add_argument("--k", type=int, default=10, help="pairs to report (default: 10)")

    similar = commands.add_parser("similar", help="cached chunks nearest a query text or file")
    query = similar.add_mutually_exclusive_group(required=True)
    query.add_argument("query", nargs="?", help="query text")
    query.add_argument("--file", help="file whose contents are the query")
    similar.add_argument("--k", type=int, default=10, help="chunks to report (default: 10)")

    ensure = commands.add_parser("ensure", help="chunk a root and embed the cache misses")
    ensure.add_argument("root", nargs="?", default=".", help="repo root to chunk and embed (default: .)")

    for sub in (dupes, pairs, similar, ensure):
        sub.add_argument("--json", action="store_true", help="one row-oriented JSON document instead of a table")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    command: str = args.command
    try:
        if command == "dupes":
            frame = embed.dupes(args.root, k=args.k)
        elif command == "pairs":
            frame = embed.pairs(k=args.k)
        elif command == "similar":
            if args.file is None:
                query: str = args.query
            else:
                # Read here rather than passing the path through: embed.similar's
                # path sniffing embeds a missing path as literal query text, and
                # an explicit --file must fail loudly instead.
                try:
                    query = Path(args.file).expanduser().read_text(encoding="utf-8")
                except OSError as exc:
                    print(f"embed: cannot read --file {args.file}: {exc}", file=sys.stderr)
                    return 1
            frame = embed.similar(query, k=args.k)
        else:  # ensure; hash/path only: text and embedding are cache payload, not report
            frame = embed.ensure(args.root).select("hash", "path")
    except embed.EmbedError as exc:
        print(exc, file=sys.stderr)
        return 1
    _emit(frame, as_json=args.json)
    return 0


if __name__ == "__main__":
    sys.exit(main())
