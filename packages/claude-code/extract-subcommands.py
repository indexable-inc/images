"""Extract the argv tokens Claude Code dispatches positionally.

Claude Code selects a subcommand from the first argv token and then re-reads
the raw `process.argv` by fixed index (`remote-control` hands
`process.argv.slice(3)` to its own parser; the entry fast paths compare
`process.argv[2]`). Anything prepended ahead of that token therefore lands
inside the subcommand's argv, and it exits with `Unknown argument`
(index#4269). packages/config-launch withholds the wrapper's flags for exactly
the tokens listed here.

Two sources, because neither alone is complete:

  help      `claude --help` prints the registered, non-hidden root commands with
            their aliases (`plugin|plugins`). This is the CLI's own answer, so a
            command added upstream shows up the next time the registry is
            regenerated.
  hidden    Root commands and entry fast paths `--help` does not print. Curated,
            because nothing enumerates them; each carries the byte pattern that
            proves it is still there, so a rename or removal fails this script
            rather than going stale.

usage: extract-subcommands.py <claude-binary> <version>
"""

import re
import subprocess
import sys
import tempfile
from pathlib import Path

# Token -> the byte pattern in the bundle that proves the token is still
# dispatched positionally. `.command("x"` is a commander registration;
# `==="x"` is an entry fast path comparing process.argv[2] / argv.slice(2)[0].
HIDDEN: dict[str, bytes] = {
    # Hidden root command, plus the entry fast path's four extra spellings.
    "remote-control": b'.command("remote-control"',
    "rc": b'==="rc"',
    "remote": b'==="remote"',
    "sync": b'==="sync"',
    "bridge": b'==="bridge"',
    # Dispatched only by the entry fast path; no commander registration.
    "daemon": b'==="daemon"',
    "import-conversations": b'.command("import-conversations',
    # Self-spawn entry points. The CLI re-execs itself for these, so they
    # normally bypass this wrapper, but a caller (a Chrome native-messaging
    # host manifest, a supervisor) can still name the wrapper.
    "--daemon-worker": b'==="--daemon-worker"',
    "--bg-pty-host": b'==="--bg-pty-host"',
    "--bg-spare": b'==="--bg-spare"',
    "--preload": b'==="--preload"',
    "--claude-in-chrome-mcp": b'==="--claude-in-chrome-mcp"',
    "--chrome-native-host": b'==="--chrome-native-host"',
    "--computer-use-mcp": b'==="--computer-use-mcp"',
}

# `claude import` is registered behind a feature gate, and its action only
# prints usage text: it neither slices argv nor breaks on a leading flag. A
# gated name must stay OUT, because withholding flags from a token that is not
# a command on this machine would silently drop the house prompt from a
# one-word prompt (`claude import`).
GATED_OUT = frozenset({"import"})

# A command entry in `--help` output: two spaces, the name, optional
# `|alias`, then argument or option placeholders.
ENTRY = re.compile(r"^ {2}(?P<names>[a-z0-9][a-z0-9|-]*)(?: |$)")


def help_commands(binary: str) -> list[str]:
    """Registered non-hidden root commands and aliases, from the CLI itself."""
    with tempfile.TemporaryDirectory() as home:
        proc = subprocess.run(
            [binary, "--help"],
            capture_output=True,
            text=True,
            timeout=120,
            check=True,
            # A scratch HOME keeps the probe off the caller's config and out of
            # the onboarding paths that would want a TTY.
            env={"HOME": home, "PATH": "/usr/bin:/bin", "CI": "1"},
        )
    lines = proc.stdout.splitlines()
    try:
        start = lines.index("Commands:")
    except ValueError as missing:
        raise SystemExit(
            "extract-subcommands: `claude --help` printed no Commands section; "
            "the help layout changed and this extractor needs rewriting"
        ) from missing
    found: list[str] = []
    for line in lines[start + 1 :]:
        if line and not line.startswith(" "):
            break
        match = ENTRY.match(line)
        if match:
            found.extend(match.group("names").split("|"))
    found = [name for name in found if name not in GATED_OUT]
    if len(found) < 5:
        raise SystemExit(
            f"extract-subcommands: parsed only {len(found)} commands from `--help`; "
            "the help layout changed and this extractor needs rewriting"
        )
    return found


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: extract-subcommands.py <claude-binary> <version>")
    binary, version = sys.argv[1], sys.argv[2]

    blob = Path(binary).read_bytes()
    missing = [token for token, proof in HIDDEN.items() if proof not in blob]
    if missing:
        raise SystemExit(
            "extract-subcommands: no dispatch evidence in the bundle for "
            f"{', '.join(missing)}. Upstream renamed or dropped them; re-read the "
            "entry dispatch and update HIDDEN in this script."
        )

    rows = [(name, "help") for name in help_commands(binary)]
    rows += [(token, "hidden") for token in HIDDEN]

    print(
        f"# claude-code positional argv tokens, extracted from Claude Code cli.js {version}"
    )
    print("# by packages/claude-code/extract-subcommands.py. Regenerate with")
    print("#   nix run .#claude-code.updateScript -- <version>")
    print("# token\tsource")
    for token, source in sorted(rows):
        print(f"{token}\t{source}")


if __name__ == "__main__":
    main()
