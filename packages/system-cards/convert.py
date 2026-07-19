"""Convert pinned system-card PDFs to markdown at build time."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import pymupdf4llm


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--pdf", action="append", default=[], metavar="SLUG=PATH")
    options = parser.parse_args()

    pdfs: dict[str, str] = {}
    for spec in options.pdf:
        slug, _, path = spec.partition("=")
        pdfs[slug] = path

    catalog = json.loads(options.catalog.read_text())
    for entry in catalog["cards"]:
        slug = str(entry["slug"])
        body = pymupdf4llm.to_markdown(pdfs[slug])
        header = "\n".join(
            [
                "---",
                f"title: {json.dumps(str(entry['title']))}",
                f"vendor: {entry['vendor']}",
                f"date: {entry['date']}",
                f"source: {entry['url']}",
                f"source_sha256: {entry['sha256']}",
                "generator: pymupdf4llm",
                "---",
                "",
            ]
        )
        target = options.out / str(entry["vendor"]) / f"{slug}.md"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(header + body)
        print(f"{slug}: {target.stat().st_size // 1024} KiB", flush=True)


if __name__ == "__main__":
    main()
