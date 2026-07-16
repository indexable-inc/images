//! `PreToolUse` guards, compiled ports of the personal `cargo-guard.py`,
//! `bash-habits-guard.py`, and `search-guard.py`.
//!
//! Each blocks a known-bad call and tells the agent the better path. Unlike the
//! Python originals (which exit 2 with a stderr message), these emit the index
//! house JSON deny (`permissionDecision: "deny"`, same channel as
//! `worktree-guard`), which both Claude Code and the Codex fork honor. Every
//! guard fails OPEN: a parse error, the wrong tool, or an unmatched command
//! returns with no output and the call proceeds.

use serde_json::Value;

use crate::DenyOutput;

fn deny(reason: String) {
    crate::emit(DenyOutput {
        hook_event_name: "PreToolUse",
        permission_decision: "deny",
        permission_decision_reason: reason,
    });
}

fn payload() -> Option<Value> {
    serde_json::from_str(&crate::read_stdin()?).ok()
}

fn command_of(payload: &Value) -> String {
    payload
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// True when `word` is a shell env-assignment prefix like `FOO=bar`.
fn is_env_assignment(word: &str) -> bool {
    regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=")
        .is_ok_and(|re| re.is_match(word))
}

/// `PreToolUse(Bash)`: block bare `cargo <sub>` inside indexable-inc/index|ix
/// (and their worktrees), steering work to nix. Nix-wrapped cargo
/// (`nix run .#run -- cargo ...`) is allowed: cargo is not the first word there.
pub fn cargo_guard() {
    let Some(payload) = payload() else { return };
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return;
    }
    let cwd = payload.get("cwd").and_then(Value::as_str).unwrap_or_default();
    // The `(/|$)` keeps `ix` from also matching `index`.
    let in_monorepo = regex::Regex::new(r"/indexable-inc/(index|ix)(/|$)")
        .is_ok_and(|re| re.is_match(cwd));
    if !in_monorepo {
        return;
    }
    let cmd = command_of(&payload);
    let first_word_is_cargo = |segment: &str| {
        segment
            .split_whitespace()
            .find(|w| !is_env_assignment(w))
            .is_some_and(|w| w == "cargo")
    };
    let any_cargo = regex::Regex::new(r"&&|\|\||;|\n|\|")
        .is_ok_and(|re| re.split(&cmd).any(first_word_is_cargo));
    if any_cargo {
        deny(
            "cargo is disabled in indexable-inc/index and /ix. Use nix: \
             `nix build .#<pkg>`, `nix run .#<name>`, `nix run .#lint`. \
             For a real passthrough use `nix run .#run -- cargo <args>`; \
             hand-edit Cargo.lock for path crates. (cargo-guard hook)"
                .to_owned(),
        );
    }
}

const GREP_PREFIXES: &[&str] = &[
    "env", "sudo", "command", "nice", "time", "xargs", "stdbuf", "nohup",
];

fn is_recursive_flag(tok: &str) -> bool {
    if tok == "--recursive" || tok.starts_with("--dereference-recursive") {
        return true;
    }
    // bundled short flags, e.g. -r, -R, -rn, -rin
    regex::Regex::new(r"^-[A-Za-z]*$").is_ok_and(|re| re.is_match(tok))
        && tok[1..].contains(['r', 'R'])
}

/// True when `stage` (a statement's first pipe stage) runs `grep` recursively so
/// it walks a tree (a `... | grep -r` reading a pipe does not traverse).
fn grep_walks_tree(stage: &str) -> bool {
    let toks: Vec<&str> = stage.split_whitespace().collect();
    let mut i = 0;
    while i < toks.len() && (is_env_assignment(toks[i]) || GREP_PREFIXES.contains(&toks[i])) {
        i += 1;
    }
    if toks.get(i) != Some(&"grep") {
        return false;
    }
    for t in &toks[i + 1..] {
        if *t == "--" {
            break; // everything after -- is operands, not flags
        }
        if is_recursive_flag(t) {
            return true;
        }
    }
    false
}

fn quoted_value_is_multiline(value: &str) -> bool {
    let (quote_offset, quote, honors_escapes, ansi_c) = match value.as_bytes() {
        [b'$', b'\'', ..] => (1, b'\'', true, true),
        [b'\'', ..] => (0, b'\'', false, false),
        [b'"', ..] => (0, b'"', true, false),
        _ => return false,
    };
    let quoted = &value.as_bytes()[quote_offset..];
    let mut escaped = false;
    let mut quote_count = 0;
    for byte in quoted {
        if escaped {
            if ansi_c && *byte == b'n' && quote_count == 1 {
                return true;
            }
            escaped = false;
            continue;
        }
        if honors_escapes && *byte == b'\\' {
            escaped = true;
            continue;
        }
        if *byte == quote {
            quote_count += 1;
        }
        if *byte == b'\n' && quote_count == 1 {
            return true;
        }
    }

    // A contraction can close a single-quoted body early and leave the shell
    // command malformed. Treat an unbalanced quoted tail as multiline when it
    // spans lines so the guard still catches the command before zsh parses it.
    value.contains('\n') && quote_count % 2 == 1
}

fn is_outside_shell_quotes(value: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for byte in value.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && quote != Some(b'\'') {
            escaped = true;
            continue;
        }
        match (quote, byte) {
            (None, b'\'' | b'"') => quote = Some(byte),
            (Some(open), close) if open == close => quote = None,
            _ => {}
        }
    }
    quote.is_none()
}

fn has_multiline_inline_gh_body(command: &str) -> bool {
    let Ok(gh_command) = regex::Regex::new(
        r"(?m)(^|(?:&&|\|\||;)[ \t]*)gh[ \t]+(?:issue|pr)[ \t]+(?:create|comment)\b",
    ) else {
        return false;
    };
    let Ok(body_option) = regex::Regex::new(r"(^|[ \t\r\n])--body(?:=|[ \t\r\n]+)") else {
        return false;
    };

    gh_command.find_iter(command).any(|gh| {
        if !is_outside_shell_quotes(&command[..gh.start()]) {
            return false;
        }
        let tail = &command[gh.end()..];
        let Some(body) = body_option
            .find_iter(tail)
            .find(|body| is_outside_shell_quotes(&tail[..body.start()]))
        else {
            return false;
        };
        let before_body = &tail[..body.start()];
        if ["&&", "||", ";"]
            .iter()
            .any(|separator| before_body.contains(separator))
        {
            return false;
        }
        quoted_value_is_multiline(&tail[body.end()..])
    })
}

/// `PreToolUse(Bash)`: block recurring bad command shapes (output-to-/dev/null,
/// recursive `grep -r`, fragile GitHub bodies, `--no-verify`). Quote/escape-aware
/// so a literal mention inside a commit message or `echo` is not a false positive.
pub fn bash_habits_guard() {
    let Some(payload) = payload() else { return };
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return;
    }
    let raw = command_of(&payload);
    let multiline_inline_gh_body = has_multiline_inline_gh_body(&raw);

    // Match operators, not literal text inside a quoted string. Neutralize
    // escaped chars, then drop quoted substrings (a real `2>/dev/null` /
    // `grep -r` is never quoted). Accepted miss: a redirection genuinely wrapped
    // in quotes or a heredoc body.
    let strip = |re: &str, s: String| {
        regex::Regex::new(re).map_or_else(|_| s.clone(), |r| r.replace_all(&s, " ").into_owned())
    };
    let cmd = strip(r#""[^"]*""#, strip(r"'[^']*'", strip(r"\\.", raw)));

    // 1. stderr-to-null / all-to-null / the `>/dev/null 2>&1` idiom.
    let to_null = [
        r"2\s*>>?\s*/dev/null",
        r"&\s*>>?\s*/dev/null",
        r">\s*/dev/null\s+2\s*>\s*&\s*1",
    ]
    .iter()
    .any(|re| regex::Regex::new(re).is_ok_and(|r| r.is_match(&cmd)));
    if to_null {
        deny(
            "Don't discard stderr/output to /dev/null - you won't see why a command \
             failed, and 223 such calls in your history silently ate the error. \
             Filter specific noise instead: `cmd 2>&1 | grep -vE '<pattern>'`, or send \
             stderr to a file you read (`cmd 2>/tmp/err`). Plain `>/dev/null` \
             (stdout only, stderr kept) is fine. (bash-habits-guard hook)"
                .to_owned(),
        );
        return;
    }

    // 2. Recursive grep that walks a tree: grep as the command of a statement's
    //    first pipe stage.
    let walks = regex::Regex::new(r"&&|\|\||;|\n").is_ok_and(|re| {
        re.split(&cmd)
            .any(|statement| grep_walks_tree(statement.split('|').next().unwrap_or("")))
    });
    if walks {
        deny(
            "Never recursive-`grep` a tree: it walks .git, result symlinks into \
             /nix/store, and node_modules, and can hit the 600s timeout. Use `rg` \
             (gitignore-aware drop-in: `rg <pat> [dir]`) or semantic search; scope \
             any plain grep to a specific subdirectory. (bash-habits-guard hook)"
                .to_owned(),
        );
        return;
    }

    // 3. Inline multiline GitHub bodies.
    if multiline_inline_gh_body {
        deny(
            "Don't pass a multiline GitHub issue or PR body through `--body`. Shell \
             quoting makes inline multiline values fragile. Pass the text through \
             `--body-file -` or a temporary file instead. (bash-habits-guard hook)"
                .to_owned(),
        );
        return;
    }

    // 4. --no-verify (bypassing git hooks).
    if regex::Regex::new(r"(^|\s)--no-verify(\s|$)").is_ok_and(|re| re.is_match(&cmd)) {
        deny(
            "Don't bypass git hooks with --no-verify. If a hook is too slow or wrong, \
             fix the hook, not the commit. If you truly must bypass it, run the command \
             yourself outside the agent. (bash-habits-guard hook)"
                .to_owned(),
        );
    }
}

/// `PreToolUse(Search)`: deny the built-in Search tool, redirect to mgrep. The
/// settings matcher is `^Search$`, but the exact-name check here is a second
/// guard so a loose matcher can never block `WebSearch`/`ToolSearch`/`mcp__*`.
pub fn search_guard() {
    let Some(payload) = payload() else { return };
    if payload.get("tool_name").and_then(Value::as_str) != Some("Search") {
        return;
    }
    deny(
        "The Search tool is disabled. Use mgrep instead: \
         `mgrep search --agentic \"<query>\" <path>` for semantic code/file search \
         (locations-only first, then Read the hits), or `rg` for exact-string \
         matches. (search-guard hook)"
            .to_owned(),
    );
}

#[cfg(test)]
mod tests {
    use super::{grep_walks_tree, has_multiline_inline_gh_body, is_recursive_flag};

    #[test]
    fn recursive_flag_detection() {
        assert!(is_recursive_flag("-r"));
        assert!(is_recursive_flag("-rn"));
        assert!(is_recursive_flag("-R"));
        assert!(is_recursive_flag("--recursive"));
        assert!(!is_recursive_flag("-n"));
        assert!(!is_recursive_flag("--include=*.rs"));
    }

    #[test]
    fn grep_tree_walk_vs_pipe() {
        assert!(grep_walks_tree("grep -r foo ."));
        assert!(grep_walks_tree("sudo grep -rn foo src"));
        assert!(grep_walks_tree("FOO=bar grep -R x"));
        // not recursive
        assert!(!grep_walks_tree("grep -n foo file"));
        // grep reading a pipe (the caller passes only the first stage, but a bare
        // non-recursive grep is fine)
        assert!(!grep_walks_tree("grep foo"));
        // -- ends flags
        assert!(!grep_walks_tree("grep -- -r"));
    }

    #[test]
    fn multiline_inline_github_bodies() {
        for command in [
            "gh issue create --body 'first\nsecond'",
            "gh issue comment 12 --body=\"first\nsecond\"",
            r"gh pr create --body $'first\nsecond'",
            "gh pr comment 12 --body 'Andrew's change\nlooks good'",
        ] {
            assert!(has_multiline_inline_gh_body(command), "{command}");
        }
    }

    #[test]
    fn safe_github_body_inputs() {
        for command in [
            "gh issue create --body 'one line'",
            "gh issue comment 12 --body-file -",
            "gh pr create --body \"$body\"",
            "gh pr comment 12 --body-file message.md",
            "gh issue edit 12 --body 'first\nsecond'",
            "echo \"gh issue create --body 'first\nsecond'\"",
            "echo \"quoted command:\ngh issue create --body 'first\nsecond'\"",
            "gh issue create --title \"literal --body 'first\nsecond'\" --body-file -",
            "gh issue create --title ok && echo --body 'first\nsecond'",
        ] {
            assert!(!has_multiline_inline_gh_body(command), "{command}");
        }
    }
}
