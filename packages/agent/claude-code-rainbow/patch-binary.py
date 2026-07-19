#!/usr/bin/env python3
"""Equal-length, count-gated byte patcher for the Claude Code Bun binary.

The load-bearing idea behind the whole "rainbow" chain lives here:

  * Claude Code ships as a prebuilt Bun single-file executable: a Mach-O/ELF
    with the minified app JS embedded as text inside it, plus a trailer Bun
    appends. Rewriting the file length (or stripping it) corrupts that trailer
    and the CLI aborts. But an EQUAL-LENGTH, in-place byte swap leaves every
    offset intact, so Bun still loads and runs. That is why every rule asserts
    len(find) == len(replace).

  * The `expect` count is the version-robustness gate. We hard-code how many
    times each `find` string must occur in the stock binary. If Anthropic ships
    a build where a string appears a different number of times, the swap might
    land in the wrong place (or miss), so we FAIL LOUDLY instead of silently
    emitting a subtly-wrong binary. Bump `expect` deliberately when the pin
    moves, having re-counted against the new stock binary.

Usage: patch-binary.py <input-binary> <mapping.json> <output-binary>
  mapping.json = [{"find": "...", "replace": "...", "expect": N}, ...]
Deterministic: no randomness, rules applied in file order.
"""

import json
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 4:
        sys.stderr.write(
            "usage: patch-binary.py <input> <mapping.json> <output>\n"
        )
        return 2

    input_path, mapping_path, output_path = sys.argv[1:4]

    with Path(mapping_path).open(encoding="ascii") as fh:
        rules = json.load(fh)

    with Path(input_path).open("rb") as fh:
        data = fh.read()

    for i, rule in enumerate(rules):
        find = rule["find"].encode("utf-8")
        replace = rule["replace"].encode("utf-8")
        expect = int(rule["expect"])

        # Gate 1: equal length keeps every downstream offset (and the Bun
        # trailer) byte-identical, which is the only reason patching is safe.
        if len(find) != len(replace):
            sys.stderr.write(
                f"rule {i}: LENGTH MISMATCH: find={rule['find']!r} "
                f"({len(find)}B) != replace={rule['replace']!r} "
                f"({len(replace)}B)\n"
            )
            return 1

        # Gate 2: observed occurrence count must match the pinned expectation,
        # or the stock binary drifted and we must not guess.
        found = data.count(find)
        if found != expect:
            sys.stderr.write(
                f"rule {i}: COUNT DRIFT for {rule['find']!r}: "
                f"expected {expect}, found {found}. The pinned binary changed; "
                f"re-count and update `expect` before trusting this swap.\n"
            )
            return 1

        data = data.replace(find, replace)
        sys.stdout.write(
            f"rule {i}: {rule['find']!r} -> {rule['replace']!r} "
            f"({found} occurrence(s), {len(find)}B each)\n"
        )

    with Path(output_path).open("wb") as fh:
        fh.write(data)

    sys.stdout.write(f"wrote {len(data)} bytes to {output_path}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
