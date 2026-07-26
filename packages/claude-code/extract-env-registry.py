#!/usr/bin/env python3
"""Extract Claude Code's env-var registry from the bundled cli.js.

The CLI is a Bun single-file executable that embeds cli.js as plain text.
Every environment variable the CLI reads is declared once in a family of
lazy ESM modules, each compiled to an export object of the shape
`<helper>(<module>,{NAME:()=>ref,...})` whose refs are assigned typed
accessors (`ref=Pe.bool()`, `Pe.str()`, `Pe.int({min:1})`, ...). This
script finds those export objects mechanically (no hardcoded minified
names) and emits one TSV row per entry: name, accessor type, module
ordinal.

The `__export` helper's minified name is not stable across releases -- it
was `nt` in 2.1.215 and `tt` in 2.1.220, which is exactly what broke the
previous version-pinned regex -- so the call target is matched as any
identifier and the discrimination is done by the shape of the object
literal instead.

An export object is treated as an env registry module when:
  * it parses as nothing but `NAME:()=>ref` entries (strict reconstruction),
  * it has at least MIN_KEYS entries,
  * at least ENV_SHAPED_FRACTION of its keys look like env var names, and
  * at least one key carries a sentinel prefix (CLAUDE_/ANTHROPIC_/OTEL_)
    or is a known sentinel name.

Identical key sets found twice (the binary embeds more than one copy of
the source) are deduplicated.

The result is then smoke-tested (MIN_TOTAL_ROWS, REQUIRED_KNOBS) because
the consumer -- checks.claude-code-knob-reference -- only pins the TSV's
version marker, so a parse that silently degraded to a handful of rows
would still satisfy it while gutting the reference it guards. Failing
here is the only place that degradation is visible.

Usage: extract-env-registry.py <path-to-claude-binary> <cli-version>
"""

import re
import sys
from pathlib import Path

SENTINEL_PREFIXES = (b"CLAUDE_", b"ANTHROPIC_", b"OTEL_")
SENTINEL_NAMES = {
    b"IS_SANDBOX",
    b"MAX_THINKING_TOKENS",
    b"USE_STAGING_OAUTH",
    b"DISABLE_TELEMETRY",
}
MIN_KEYS = 30
ENV_SHAPED_FRACTION = 0.9
# How far past a module's export object to look for its accessor
# definitions (they live in the module's own init function, right after).
DEF_WINDOW = 1_000_000

# Smoke test on the assembled result rather than on any one module, so a
# layout change that still matches *something* cannot pass silently.
# 2.1.215 yielded 812 rows and 2.1.220 yields 830 across 8 modules; the
# floor is set well under that but far above the handful a degenerate
# match would produce.
MIN_TOTAL_ROWS = 500
# Long-lived, publicly documented knobs. Not the registry (that stays
# derived from the binary) -- just names whose disappearance means the
# parse went wrong, not that upstream dropped the variable.
REQUIRED_KNOBS = {
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CONFIG_DIR",
    "DISABLE_TELEMETRY",
    "HTTP_PROXY",
    "MAX_THINKING_TOKENS",
}

# Any `<ident>(<ident>,{` call. The helper name is deliberately unpinned --
# pinning it is what broke on 2.1.220 -- so this matches broadly and the
# filters in `scan` do the discrimination.
EXPORT_OPEN = re.compile(rb"[A-Za-z0-9_$]+\([A-Za-z0-9_$]+,\{")
ENTRY = re.compile(rb"([A-Za-z_$][A-Za-z0-9_$]*):\(\)=>([A-Za-z0-9_$]+)")
ENV_SHAPED = re.compile(rb"^_?[A-Za-z][A-Za-z0-9_]*$")


def is_env_shaped(key: bytes) -> bool:
    if not ENV_SHAPED.match(key):
        return False
    return key.upper() == key or b"_" in key


def accessor_for(data: bytes, ref: bytes, start: int) -> str:
    window = data[start : start + DEF_WINDOW]
    m = re.search(
        rb"[,{;)(]" + re.escape(ref) + rb"=([A-Za-z0-9_$]+\.[A-Za-z0-9_$]+)\(",
        window,
    )
    if m is None:
        return "?"
    # balanced-paren scan for the accessor's argument list
    i = m.end()
    depth = 1
    while i < len(window) and depth > 0:
        depth += {ord("("): 1, ord(")"): -1}.get(window[i], 0)
        i += 1
    args = window[m.end() : i - 1].decode("utf-8", "replace")
    method = m.group(1).split(b".")[1].decode()
    return method + (f"({args})" if args else "()")


class Scan:
    """Accepted modules plus why the near misses were rejected.

    The rejection tally is what turns "the bundle layout changed" into a
    lead: which of the four filters is now doing the rejecting says which
    part of the shape moved.
    """

    def __init__(self) -> None:
        self.modules: list[list[tuple[bytes, bytes]]] = []
        self.rejected: dict[str, int] = {}
        self.near_misses: list[tuple[int, str, str]] = []

    def reject(
        self,
        filt: str,
        entry_count: int,
        sample: list[bytes],
        detail: str | None = None,
    ) -> None:
        # Tally by filter, not by detail: a detail carrying per-candidate
        # counts would make every rejection its own bucket and bury the shape.
        self.rejected[filt] = self.rejected.get(filt, 0) + 1
        keys = ", ".join(k.decode("utf-8", "replace") for k in sample[:5])
        self.near_misses.append((entry_count, detail or filt, keys))

    def diagnosis(self) -> str:
        tally = ", ".join(f"{n}x {filt}" for filt, n in sorted(self.rejected.items()))
        best = sorted(self.near_misses, reverse=True)[:5]
        lines = [f"  {n:5d} entries  {detail}: {keys}" for n, detail, keys in best]
        header = f"rejected candidates: {tally or 'none'}"
        if not lines:
            return header
        return "\n".join([header, "largest near misses:", *lines])


def scan(data: bytes) -> Scan:
    result = Scan()
    seen: set[frozenset[bytes]] = set()
    for open_match in EXPORT_OPEN.finditer(data):
        start = open_match.end()
        end = data.find(b"});", start)
        if end == -1 or end - start > 300_000:
            continue
        block = data[start:end]
        entries = [(m.group(1), m.group(2)) for m in ENTRY.finditer(block)]
        keys = [k for k, _ in entries]
        if len(entries) < MIN_KEYS:
            continue
        # strict reconstruction: the block must be entries and commas only
        rebuilt = b",".join(m.group(0) for m in ENTRY.finditer(block))
        if rebuilt != block:
            result.reject("not all `NAME:()=>ref` entries", len(entries), keys)
            continue
        shaped = sum(1 for k in keys if is_env_shaped(k))
        if shaped / len(keys) < ENV_SHAPED_FRACTION:
            result.reject(
                "keys not env-shaped",
                len(entries),
                keys,
                detail=f"only {shaped}/{len(keys)} keys env-shaped",
            )
            continue
        if not any(
            k.startswith(SENTINEL_PREFIXES) or k in SENTINEL_NAMES for k in keys
        ):
            result.reject("no sentinel key", len(entries), keys)
            continue
        key_set = frozenset(keys)
        if key_set in seen:
            continue
        seen.add(key_set)
        result.modules.append(
            [(k, accessor_for(data, r, start).encode()) for k, r in entries]
        )
    return result


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__.strip().splitlines()[-1])
    binary, version = sys.argv[1], sys.argv[2]
    data = Path(binary).read_bytes()
    result = scan(data)
    if not result.modules:
        sys.exit(
            f"no env registry modules found in {binary} (cli.js {version}).\n"
            f"Expected at least one `<helper>(<mod>,{{NAME:()=>ref,...}})` object"
            f" with >={MIN_KEYS} entries, >={ENV_SHAPED_FRACTION:.0%} env-shaped"
            f" keys, and a CLAUDE_/ANTHROPIC_/OTEL_ key.\n"
            f"{result.diagnosis()}\n"
            "Re-read the bundle around a known knob"
            " (`rg -ao 'MAX_THINKING_TOKENS:\\(\\)=>[A-Za-z0-9_$]+' <binary>`)"
            " and adjust the shape above to match."
        )

    rows = sorted(
        (name.decode(), typ.decode(), i)
        for i, entries in enumerate(result.modules)
        for name, typ in entries
    )
    names = {name for name, _, _ in rows}
    if len(rows) < MIN_TOTAL_ROWS:
        sys.exit(
            f"env registry parsed to only {len(rows)} rows across"
            f" {len(result.modules)} modules (expected >={MIN_TOTAL_ROWS});"
            " the match degraded rather than failing outright.\n"
            f"{result.diagnosis()}"
        )
    absent = sorted(REQUIRED_KNOBS - names)
    if absent:
        sys.exit(
            f"env registry is missing knobs that always exist: {', '.join(absent)}."
            f" Got {len(rows)} rows across {len(result.modules)} modules, so some"
            " registry module is being rejected rather than the whole scan.\n"
            f"{result.diagnosis()}"
        )

    print(
        "# generated by packages/claude-code/extract-env-registry.py"
        f" from Claude Code cli.js {version}"
    )
    print(
        "# regenerate: nix build .#claude-code.envRegistry &&"
        " cp result packages/claude-code/env-registry.tsv"
    )
    print("# name\ttype\tmodule")
    for name, typ, module in rows:
        print(f"{name}\t{typ}\t{module}")


if __name__ == "__main__":
    main()
