#!/usr/bin/env python3
"""PreToolUse(Bash) guard: steer toward the repo's modern tooling.

Blocks grep / find / cargo / `git checkout` / `rg -rn` only when they are the
command being *executed*, not when the word merely appears as a substring of an
argument, a quoted string, a path component, or a commit message.

Detection tokenizes the command with shlex (quote- and operator-aware) and
inspects only tokens in "command position": the first word, the word after a
shell separator (| || & && ; ( ), and the word a wrapper (sudo/env/xargs/...)
defers to. This is what makes it quote-correct -- e.g. `echo "a; cargo b"` is a
single token and never reaches command position.

Heredoc bodies are stripped before tokenizing: a heredoc carries data (a
script, a regex, a commit message), and shlex would otherwise treat its
punctuation as shell operators and put words like `grep` back in command
position. Transcript analysis found agents writing Python through `cat
<<'EOF'` denied for the *content* of the file they were writing.

Known, deliberate gaps: commands hidden inside `bash -c "..."`/backticks are
not introspected, and `git -C dir checkout` is not matched (subcommand isn't
adjacent). Both mirror the previous regex behavior. The heredoc stripper can
misread `<<` in arithmetic or string data as a heredoc start and drop the
following lines from the scan; that fails open (allow), never a false block.
"""

import json
import re
import shlex
import sys

PUNCT = set("();<>|&")
WRAPPERS = {
    "sudo", "doas", "env", "xargs", "time", "nice",
    "nohup", "command", "stdbuf", "setsid", "ionice", "chrt",
}
ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
CARGO_LOCKFILE_OK = {"add", "remove", "update", "generate-lockfile"}
HEREDOC = re.compile(r"<<-?\s*(?:'(\w+)'|\"(\w+)\"|\\?(\w+))")


def strip_heredocs(cmd: str) -> str:
    """Drop heredoc bodies (and their delimiter lines) from the command text.

    Bodies are data, not shell code; leaving them in lets their punctuation
    fabricate command positions for the tokenizer. Multiple heredocs on one
    line attach their bodies in order (FIFO). `<<-` delimiters may be
    tab-indented, per POSIX.
    """
    out_lines = []
    pending = []  # (delimiter, allows_tab_indent), in body order
    for line in cmd.split("\n"):
        if pending:
            delim, dash = pending[0]
            candidate = line.lstrip("\t") if dash else line
            if candidate == delim:
                pending.pop(0)
            continue
        for m in HEREDOC.finditer(line):
            delim = next(g for g in m.groups() if g)
            dash = line[m.start() : m.end()].startswith("<<-")
            pending.append((delim, dash))
        out_lines.append(line)
    return "\n".join(out_lines)


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
    """'sep' introduces a new command; 'redir' is followed by a filename."""
    if tok and all(c in PUNCT for c in tok):
        return "redir" if ("<" in tok or ">" in tok) else "sep"
    return None


def command_indices(tokens):
    """Indices of tokens that are in command position."""
    out, expect = [], True
    for i, tok in enumerate(tokens):
        kind = op_kind(tok)
        if kind == "sep":
            expect = True
            continue
        if kind == "redir":
            expect = False  # next token is a redirect target, not a command
            continue
        if not expect:
            continue
        if ASSIGN.match(tok):       # leading FOO=bar env assignment
            continue
        if tok.startswith("-"):     # option to a wrapper (e.g. sudo -u foo)
            continue
        out.append(i)
        base = tok.rsplit("/", 1)[-1]
        expect = base in WRAPPERS   # keep scanning past sudo/env/xargs/...
    return out


def next_word(tokens, start):
    """First non-option, non-operator token at/after `start` (a subcommand)."""
    for tok in tokens[start:]:
        if op_kind(tok):
            return None
        if tok.startswith(("-", "+")):
            continue
        return tok
    return None


def main() -> None:
    try:
        cmd = json.load(sys.stdin).get("tool_input", {}).get("command", "")
    except (json.JSONDecodeError, ValueError):
        sys.exit(0)
    if not cmd:
        sys.exit(0)

    cmd = strip_heredocs(cmd)

    try:
        lex = shlex.shlex(cmd, posix=True, punctuation_chars=True)
        lex.whitespace_split = True
        tokens = list(lex)
    except ValueError:
        # Unbalanced quotes etc. -- the shell would reject it too; allow.
        sys.exit(0)

    for i in command_indices(tokens):
        base = tokens[i].rsplit("/", 1)[-1]
        if base == "rg":
            for tok in tokens[i + 1:]:
                if op_kind(tok):
                    break
                if tok == "-rn":
                    deny("Use rg -n instead of rg -rn.")
        if base == "grep":
            deny("Use rg (ripgrep) instead of grep.")
        if base == "find":
            deny("Use fd instead of find.")
        if base == "cargo":
            # Lockfile management is allowed: there is no nix-native way to
            # regenerate Cargo.lock, so adding/removing/updating workspace deps
            # needs bare cargo. Everything else (build/check/clippy/test/run/
            # bench/nextest) still goes through nix, which owns the toolchain,
            # full dependency closure, and lints.
            if next_word(tokens, i + 1) not in CARGO_LOCKFILE_OK:
                deny(
                    "Use the repo-owned Nix build or check, not cargo directly. "
                    "Only lockfile management is allowed (cargo add/remove/"
                    "update/generate-lockfile); build/check/clippy/test/run go "
                    "through nix (`nix build .#<pkg>` and the flake checks), "
                    "which pin the toolchain, dependency closure, and lint gates."
                )
        if base == "git" and next_word(tokens, i + 1) == "checkout":
            deny(
                "Use git switch (branches) or git restore (files) instead of "
                "git checkout."
            )

    sys.exit(0)


if __name__ == "__main__":
    main()
