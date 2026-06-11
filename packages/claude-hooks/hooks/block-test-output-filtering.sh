#!/usr/bin/env python3
"""PreToolUse(Bash) guard: keep full test/typecheck/biome output intact.

Blocks piping a noisy *producer* (test run, typechecker, biome/linter, or a
Nix checks/test build) into a *lossy filter* (head/tail/grep/rg/sed/awk/...).
The Bash tool already spools large output to disk, so filtering at the pipe
only throws away the complete capture before the model ever sees it, which
leads to fixes based on a truncated error.

Detection reuses the command-position idea from enforce-modern-tools.sh: the
command is tokenized with shlex (quote- and operator-aware), split into
pipeline stages on `|`/`|&` only (never `&&`/`||`/`;`, which start a fresh
command), and each stage's real command is resolved past env assignments,
wrappers (sudo/env/xargs/...), and options. A stage is denied when it is a
filter whose pipe-connected upstream in the same pipeline contains a producer.

Deliberate, safe-by-design gaps (all fail open, i.e. allow):
  - commands hidden inside `bash -c "..."` / backticks are not introspected;
  - exotic `sudo -u NAME <cmd>` resolves to NAME, so an unusual wrapper-with-
    option form may miss the real command (false negative, never a false block);
  - a bare `grep file` or `rg file` with no upstream producer is allowed -- that
    is exactly the recommended workaround (filter the <persisted_output> file).
"""

import json
import re
import shlex
import sys

PUNCT = set("();<>|&")
WRAPPERS = {
    "sudo", "doas", "env", "xargs", "time", "nice",
    "nohup", "command", "stdbuf", "setsid", "ionice", "chrt",
    "direnv",  # `direnv exec . <cmd>` is the repo's standard runner prefix
}
ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")

# Lossy filters: they truncate, drop context, or hide a non-zero exit behind an
# empty match. tee/cat are intentionally absent (full passthrough, not lossy).
FILTERS = {
    "head", "tail", "grep", "egrep", "fgrep", "rg", "ag", "ack",
    "sed", "awk", "gawk", "cut", "wc", "less", "more", "most",
}

# Producers whose full diagnostic output we want to preserve, regardless of args.
PRODUCER_BASES = {"biome", "tsc", "jest", "vitest", "pytest", "mypy", "pyright"}

# JS package managers: a producer only when the script/target is a test or
# typecheck (e.g. `npm run test`, `bun run typecheck`, `pnpm test`).
JS_RUNNERS = {"npm", "pnpm", "yarn", "bun", "npx", "bunx", "deno"}
TEST_WORDS = {
    "test", "typecheck", "type-check", "check-types", "tsc",
    "biome", "jest", "vitest", "lint",
}


def deny(reason: str) -> None:
    json.dump(
        {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        },
        sys.stdout,
    )
    sys.exit(0)


def op_kind(tok: str):
    """Classify a punctuation token: pipe | break (new command) | redir."""
    if tok and all(c in PUNCT for c in tok):
        if "<" in tok or ">" in tok:
            return "redir"
        if tok in ("|", "|&"):
            return "pipe"
        return "break"  # && || ; & ( )
    return None


def stage_command(tokens):
    """Resolve a stage's real command base (used for filter detection).

    Skips leading env assignments, wrapper commands, and options. Filters appear
    bare at the head of a downstream stage (`| tail`, `| rg foo`), so resolving
    the first real word is enough; producer detection scans the whole stage.
    """
    skip_next = False
    for tok in tokens:
        if op_kind(tok) == "redir":
            skip_next = True  # the following token is a redirect target
            continue
        if skip_next:
            skip_next = False
            continue
        if ASSIGN.match(tok) or tok.startswith("-"):
            continue
        candidate = tok.rsplit("/", 1)[-1]
        if candidate in WRAPPERS:
            continue  # keep scanning for the wrapped command
        return candidate
    return None


def stage_is_producer(tokens) -> bool:
    """Detect a producer anywhere in the stage's tokens.

    Scanning the whole stage (rather than only the command-position word) keeps
    detection robust to the repo's positional-arg runners: `direnv exec . nix
    build .#checks ...` and `timeout 600 cargo nextest run` both wrap the real
    command behind tokens a simple command-position resolver would stop on.
    Quoted strings stay single tokens, so `echo "cargo test"` never matches.
    """
    words = set(tokens)
    if words & PRODUCER_BASES:
        return True
    if "cargo" in words and words & {"test", "nextest"}:
        return True
    if "just" in words and words & {"test", "typecheck", "lint"}:
        return True
    if words & JS_RUNNERS and words & TEST_WORDS:
        return True
    if "nix" in words and any(
        ("checks" in t) or ("rust-test" in t) or ("required-ci" in t)
        for t in tokens
    ):
        return True
    return False


def split_pipeline_stages(tokens):
    """Yield (stage_tokens, joined_by_pipe) split on pipe/break operators.

    `joined_by_pipe` is True when the operator *preceding* this stage was a
    pipe, i.e. the stage consumes the previous stage's stdout.
    """
    stages = []
    current = []
    joined = False  # the first stage is never pipe-fed
    for tok in tokens:
        kind = op_kind(tok)
        if kind in ("pipe", "break"):
            stages.append((current, joined))
            current = []
            joined = kind == "pipe"
            continue
        current.append(tok)
    stages.append((current, joined))
    return stages


def main() -> None:
    try:
        cmd = json.load(sys.stdin).get("tool_input", {}).get("command", "")
    except (json.JSONDecodeError, ValueError):
        sys.exit(0)
    if not cmd:
        sys.exit(0)

    try:
        lex = shlex.shlex(cmd, posix=True, punctuation_chars=True)
        lex.whitespace_split = True
        # `#` is not a comment here: it carries the flake ref in `nix build .#x`.
        lex.commenters = ""
        tokens = list(lex)
    except ValueError:
        # Unbalanced quotes etc. -- the shell would reject it too; allow.
        sys.exit(0)

    pipe_has_producer = False  # producer seen earlier in the current pipeline
    for stage_tokens, joined_by_pipe in split_pipeline_stages(tokens):
        if not joined_by_pipe:
            pipe_has_producer = False  # `break` op started a fresh command

        base = stage_command(stage_tokens)
        if joined_by_pipe and pipe_has_producer and base in FILTERS:
            deny(
                "Do not pipe test, typecheck, or biome output to "
                "head/tail/grep/etc.\n\n"
                "The Bash tool spools large output to disk automatically. Run "
                "the full command without filtering so the complete output is "
                "captured. If the output is too long, use the "
                "<persisted_output> path from the tool result and filter that "
                "file instead."
            )
        if stage_is_producer(stage_tokens):
            pipe_has_producer = True

    sys.exit(0)


if __name__ == "__main__":
    main()
