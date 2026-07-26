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
    regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").is_ok_and(|re| re.is_match(word))
}

/// `PreToolUse(Bash)`: block bare `cargo <sub>` inside indexable-inc/index|ix
/// (and their worktrees), steering work to nix. Nix-wrapped cargo
/// (`nix run .#run -- cargo ...`) is allowed: cargo is not the first word there.
pub fn cargo_guard() {
    let Some(payload) = payload() else { return };
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return;
    }
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // The `(/|$)` keeps `ix` from also matching `index`.
    let in_monorepo =
        regex::Regex::new(r"/indexable-inc/(index|ix)(/|$)").is_ok_and(|re| re.is_match(cwd));
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

/// `PreToolUse(Bash)`: block recurring bad command shapes (output-to-/dev/null,
/// recursive `grep -r`, `--no-verify`). Quote/escape-aware so a literal mention
/// inside a commit message or `echo` is not a false positive.
pub fn bash_habits_guard() {
    let Some(payload) = payload() else { return };
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return;
    }
    let raw = command_of(&payload);

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

    // 3. --no-verify (bypassing git hooks).
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

// --- git-guard ---

/// Statement prefixes that wrap another command; skipped when looking for the
/// command word so `sudo git reset --hard` is still seen as git.
const CMD_PREFIXES: &[&str] = &[
    "env", "sudo", "command", "nice", "time", "stdbuf", "nohup", "doas",
];

/// Git global options that consume the following token as their value, so the
/// subcommand scan does not mistake that value for the subcommand.
const GIT_VALUE_OPTS: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace"];

/// One shell statement: its tokens, quote-aware and with quotes removed.
type Statement = Vec<String>;

/// Split a command line into statements on the unquoted operators that begin a
/// new command word (`;`, newline, `&&`, `||`, `|`, `(`, `)`), tokenizing each
/// on unquoted whitespace and dropping the quote characters.
///
/// Hand-rolled rather than regex-split because the paths this guard resolves
/// are frequently quoted (`git -C "/Users/a b/ix" reset --hard`), and the
/// quote-stripping trick `bash_habits_guard` uses would eat them.
fn statements(cmd: &str) -> Vec<Statement> {
    let (mut out, mut stmt, mut tok) = (Vec::new(), Statement::new(), String::new());
    let (mut quoted, mut single, mut double) = (false, false, false);
    let mut chars = cmd.chars().peekable();
    let flush_tok = |tok: &mut String, stmt: &mut Statement, quoted: &mut bool| {
        if !tok.is_empty() || *quoted {
            stmt.push(std::mem::take(tok));
        }
        *quoted = false;
    };
    while let Some(c) = chars.next() {
        match c {
            '\\' if !single => {
                if let Some(n) = chars.next() {
                    tok.push(n);
                }
            }
            '\'' if !double => {
                single = !single;
                quoted = true;
            }
            '"' if !single => {
                double = !double;
                quoted = true;
            }
            _ if single || double => tok.push(c),
            ';' | '\n' | '(' | ')' => {
                flush_tok(&mut tok, &mut stmt, &mut quoted);
                if !stmt.is_empty() {
                    out.push(std::mem::take(&mut stmt));
                }
            }
            '&' | '|' => {
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                flush_tok(&mut tok, &mut stmt, &mut quoted);
                if !stmt.is_empty() {
                    out.push(std::mem::take(&mut stmt));
                }
            }
            c if c.is_whitespace() => flush_tok(&mut tok, &mut stmt, &mut quoted),
            _ => tok.push(c),
        }
    }
    flush_tok(&mut tok, &mut stmt, &mut quoted);
    if !stmt.is_empty() {
        out.push(stmt);
    }
    out
}

/// True when any token is the long flag `long`, or a bundled short flag group
/// (`-fd`, `-xfd`) containing `short`. Scanning stops at `--`, after which
/// everything is a pathspec.
fn has_flag(args: &[String], short: char, long: &str) -> bool {
    args.iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| {
            a == long
                || (a.starts_with('-')
                    && !a.starts_with("--")
                    && a.len() > 1
                    && a[1..].chars().all(char::is_alphanumeric)
                    && a[1..].contains(short))
        })
}

/// Classify a git subcommand as one that can discard uncommitted work, naming
/// what it does. `None` means "cannot lose working-tree state".
///
/// A closed, enumerated set. Commands that already refuse to run against a
/// dirty tree (`merge`, `rebase`, `cherry-pick`) are deliberately absent: git
/// itself is the guard there, and denying them would be noise.
fn discards_worktree(sub: &str, args: &[String]) -> Option<&'static str> {
    let flag = |s: char, l: &str| has_flag(args, s, l);
    match sub {
        // --hard throws the tree away; --merge and --keep discard a subset.
        "reset" if args.iter().any(|a| ["--hard", "--merge", "--keep"].contains(&a.as_str())) => {
            Some("overwrites the working tree from a commit, discarding every uncommitted change")
        }
        // Branch-switching checkout refuses to clobber; the pathspec form
        // (`-- <path>`, or a bare `.`) and -f do not.
        "checkout"
            if flag('f', "--force")
                || args.iter().any(|a| a == "--" || a == ".") =>
        {
            Some("restores the named paths from the index or a commit, discarding their uncommitted changes")
        }
        "switch" if flag('f', "--force") || args.iter().any(|a| a == "--discard-changes") => {
            Some("switches branches discarding local changes")
        }
        // `restore --staged` alone only rewrites the index; anything else
        // (default, or an explicit --worktree) rewrites the working tree.
        "restore"
            if !args.iter().any(|a| a == "--staged")
                || args.iter().any(|a| a == "--worktree" || a == "-W") =>
        {
            Some("restores paths over the working tree, discarding their uncommitted changes")
        }
        // -n/--dry-run is the safe form; without -f git clean refuses anyway.
        "clean" if flag('f', "--force") && !flag('n', "--dry-run") => {
            Some("deletes untracked files, which are never recoverable from the object database")
        }
        // `stash list|show|create` inspect or snapshot without touching the
        // tree; every other form moves work out of it or destroys saved work.
        "stash"
            if !matches!(
                args.iter().find(|a| !a.starts_with('-')).map(String::as_str),
                Some("list" | "show" | "create")
            ) =>
        {
            Some("moves uncommitted work off the working tree (or destroys already-stashed work)")
        }
        _ => None,
    }
}

/// `org/repo` from a checkout's origin URL, for the `/tmp/worktree/<org>/<repo>`
/// convention the suggested command spells out. Falls back to the directory
/// name when origin is missing or unparseable.
fn origin_slug(git: &str, top: &str) -> String {
    let url = std::process::Command::new(git)
        .args(["-C", top, "remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    regex::Regex::new(r"[:/]([^/:]+)/([^/]+?)(?:\.git)?/?$")
        .ok()
        .and_then(|re| re.captures(url.trim()).map(|c| format!("{}/{}", &c[1], &c[2])))
        .or_else(|| {
            std::path::Path::new(top)
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "repo".to_owned())
}

/// A parsed `git` invocation: the directory it runs in (payload cwd, composed
/// with every `-C`), its subcommand, and the arguments after it.
struct GitCall {
    dir: std::path::PathBuf,
    sub: String,
    args: Vec<String>,
}

/// Parse the tokens after the `git` command word. `base` is the directory the
/// statement runs in; each `-C` composes onto it, exactly as git does.
fn parse_git_call(rest: &[String], base: &std::path::Path) -> Option<GitCall> {
    let mut dir = base.to_path_buf();
    let mut i = 0;
    while let Some(a) = rest.get(i) {
        if a == "-C" {
            if let Some(v) = rest.get(i + 1) {
                dir = resolve(&dir, v);
            }
            i += 2;
        } else if GIT_VALUE_OPTS.contains(&a.as_str()) {
            i += 2;
        } else if a.starts_with('-') {
            i += 1;
        } else {
            // `rest.get(i + 1..)` and not `rest[i + 1..]`: a trailing
            // value-taking option (`git -c`) leaves `i` past the end.
            return Some(GitCall {
                dir,
                sub: a.clone(),
                args: rest.get(i + 1..).unwrap_or_default().to_vec(),
            });
        }
    }
    None
}

/// The refusal text. Names every entry that would be lost, the snapshot that
/// does not touch the tree, and the worktree to do the work in instead.
struct Refusal<'a> {
    git: &'a str,
    top: &'a str,
    sub: &'a str,
    /// What the subcommand does to the working tree, from `discards_worktree`.
    effect: &'a str,
    /// `git status --porcelain` lines, one per entry that would be lost.
    dirty: &'a [String],
}

fn deny_message(refusal: &Refusal<'_>) -> String {
    let Refusal {
        git,
        top,
        sub,
        effect,
        dirty,
    } = *refusal;
    let shown = dirty
        .iter()
        .take(10)
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let more = if dirty.len() > 10 {
        format!("\n    ... and {} more", dirty.len() - 10)
    } else {
        String::new()
    };
    let slug = origin_slug(git, top);
    let count = dirty.len();
    format!(
        "Refusing `git {sub}` in {top}.\n\n\
         That checkout is shared by every agent session on this machine, it currently \
         holds {count} entries of uncommitted work, and `git {sub}` {effect}. None of it is \
         staged, so there is no blob in the object database and no way back (ENG-9964: \
         exactly this command destroyed 7 uncommitted lines belonging to another session, \
         recovered only because a nix eval had happened to copy the dirty tree into the \
         store).\n\n\
         Would be lost:\n{shown}{more}\n\n\
         Snapshot it first, which does not touch the working tree:\n\
         \x20   git -C {top} branch rescue/$(date +%Y-%m-%d-%H%M) \"$(git -C {top} stash create)\"\n\n\
         Then do the work in a worktree of your own, where destructive commands are yours \
         to run:\n\
         \x20   git -C {top} worktree add /tmp/worktree/{slug}/<name> -b <branch> origin/main\n\
         \x20   git -C /tmp/worktree/{slug}/<name> submodule update --init --recursive\n\n\
         (git-guard hook, ENG-9964)"
    )
}

/// Judge one parsed git call. `Some(reason)` means refuse.
///
/// This is where the guard stops failing open. Up to the point where the target
/// is known to be a protected checkout, anything unreadable means "not our
/// business" and returns `None`. After it, an unreadable `git status` is a
/// refusal, because the alternative is allowing the one command whose failure
/// mode is unrecoverable data loss (ENG-9964).
fn judge(git: &str, protected: &[String], call: &GitCall) -> Option<String> {
    let effect = discards_worktree(&call.sub, &call.args)?;
    let sub = &call.sub;

    let Some(top) = crate::git_rev_parse(git, &call.dir, "--show-toplevel") else {
        // Not a readable repo. Only a textual match on the protected globs is
        // left to go on; refuse that rather than guess.
        // A path that is not valid UTF-8 cannot match a UTF-8 glob pattern, so
        // there is nothing here this guard could be protecting.
        let dir = call.dir.to_str()?;
        return crate::matches_protected(dir, protected).then(|| {
            format!(
                "Refusing `git {sub}` in {dir}: that path is a protected primary checkout \
                 but git could not read it, so this guard cannot tell what would be lost. \
                 Run the command in your own worktree instead. (git-guard hook, ENG-9964)"
            )
        });
    };
    // A linked worktree is the caller's own; its dirt is theirs to discard.
    let gd = crate::git_rev_parse(git, &call.dir, "--git-dir")?;
    let common = crate::git_rev_parse(git, &call.dir, "--git-common-dir")?;
    if gd != common || !crate::matches_protected(&top, protected) {
        return None;
    }

    let status = std::process::Command::new(git)
        .args(["-C", &top, "status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success());
    let Some(out) = status else {
        return Some(format!(
            "Refusing `git {sub}` in {top}: it is a shared primary checkout and `git status` \
             could not be read, so this guard cannot tell whether another session has \
             uncommitted work here. Use your own worktree. (git-guard hook, ENG-9964)"
        ));
    };
    let dirty: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    // Nothing uncommitted means nothing for this command to destroy. Not a
    // silent no-op: the checkout is genuinely empty of work to lose.
    if dirty.is_empty() {
        return None;
    }
    Some(deny_message(&Refusal {
        git,
        top: &top,
        sub,
        effect,
        dirty: &dirty,
    }))
}

/// `PreToolUse(Bash)`: refuse a destructive git command aimed at a shared
/// primary checkout that currently holds uncommitted work.
///
/// Why a hook and not a git hook: git has no `pre-reset`, `pre-checkout`,
/// `pre-clean` or `pre-stash`, `post-checkout` runs after the damage, and
/// `reference-transaction` never fires for `git clean` or for a `reset --hard`
/// that does not move HEAD. `PreToolUse` is the only seam that sees the command
/// before it runs (ENG-9964).
pub fn git_guard() {
    if crate::flag_set("CLAUDE_CODE_DISABLE_GIT_GUARD") {
        return;
    }
    let Some(payload) = payload() else { return };
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return;
    }
    let protected = crate::primary_checkouts();
    if protected.is_empty() {
        return;
    }
    let git = std::env::var("IX_GIT").unwrap_or_else(|_| "git".to_owned());
    let mut cwd = std::path::PathBuf::from(
        payload
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map_or_else(
                || std::env::var("PWD").unwrap_or_else(|_| ".".to_owned()),
                str::to_owned,
            ),
    );

    for stmt in statements(&command_of(&payload)) {
        let mut it = stmt
            .iter()
            .skip_while(|w| is_env_assignment(w) || CMD_PREFIXES.contains(&w.as_str()));
        let Some(head) = it.next() else { continue };
        let rest: Vec<String> = it.cloned().collect();
        // Track `cd` across the chain: `cd <primary> && git reset --hard` must
        // be judged against the cd target, not the payload cwd.
        if head == "cd" {
            if let Some(target) = rest.iter().find(|a| !a.starts_with('-')) {
                cwd = resolve(&cwd, target);
            }
            continue;
        }
        if head != "git" {
            continue;
        }
        let Some(call) = parse_git_call(&rest, &cwd) else {
            continue;
        };
        if let Some(reason) = judge(&git, &protected, &call) {
            deny(reason);
            return;
        }
    }
}

/// Join `arg` onto `base` unless it is already absolute.
fn resolve(base: &std::path::Path, arg: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(arg);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        discards_worktree, grep_walks_tree, has_flag, is_recursive_flag, parse_git_call, statements,
    };

    fn toks(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn statement_splitting_is_quote_aware() {
        assert_eq!(
            statements("cd /a && git reset --hard"),
            vec![toks(&["cd", "/a"]), toks(&["git", "reset", "--hard"])]
        );
        // A quoted path with a space survives as one token.
        assert_eq!(
            statements(r#"git -C "/Users/a b/ix" clean -fd"#),
            vec![toks(&["git", "-C", "/Users/a b/ix", "clean", "-fd"])]
        );
        // Pipes, semicolons and newlines all start a new statement.
        assert_eq!(statements("a | b ; c\nd").len(), 4);
        // An operator inside quotes is literal text, not a split.
        assert_eq!(statements("echo 'a && b'"), vec![toks(&["echo", "a && b"])]);
    }

    #[test]
    fn bundled_short_flags() {
        assert!(has_flag(&toks(&["-fd"]), 'f', "--force"));
        assert!(has_flag(&toks(&["-xfd"]), 'f', "--force"));
        assert!(has_flag(&toks(&["--force"]), 'f', "--force"));
        assert!(!has_flag(&toks(&["-d"]), 'f', "--force"));
        // Scanning stops at `--`: a pathspec named -f is not a flag.
        assert!(!has_flag(&toks(&["--", "-f"]), 'f', "--force"));
    }

    #[test]
    fn git_call_parsing() {
        let base = std::path::Path::new("/base");
        let call = |v: &[&str]| parse_git_call(&toks(v), base);
        // -C composes onto the payload cwd; global options are stepped over.
        let c = call(&["-C", "/repo", "reset", "--hard"]).expect("parsed");
        assert_eq!(c.dir, std::path::Path::new("/repo"));
        assert_eq!(c.sub, "reset");
        assert_eq!(c.args, toks(&["--hard"]));
        // A value-taking global option must not be read as the subcommand.
        let c = call(&["-c", "core.pager=cat", "clean", "-fd"]).expect("parsed");
        assert_eq!(c.sub, "clean");
        assert_eq!(c.dir, base);
        // A relative -C resolves against the payload cwd.
        assert_eq!(
            call(&["-C", "sub", "stash"]).expect("parsed").dir,
            std::path::Path::new("/base/sub")
        );
        // Regression: a trailing value-taking option leaves the scan index past
        // the end of the argument list, which used to panic on `rest[i..]`.
        assert!(call(&["-c"]).is_none());
        assert!(call(&["-C"]).is_none());
        assert!(call(&[]).is_none());
    }

    #[test]
    fn destructive_git_classification() {
        let d = |sub: &str, a: &[&str]| discards_worktree(sub, &toks(a)).is_some();
        // The five shapes ENG-9964 names.
        assert!(d("reset", &["--hard", "HEAD~1"]));
        assert!(d("checkout", &["--", "."]));
        assert!(d("clean", &["-fd"]));
        assert!(d("stash", &[]));
        assert!(d("restore", &["."]));
        // Safe siblings must not be denied, or the guard becomes noise.
        assert!(!d("reset", &["--soft", "HEAD~1"]));
        assert!(!d("reset", &[]));
        assert!(!d("checkout", &["-b", "feature"]));
        assert!(!d("checkout", &["main"]));
        assert!(!d("clean", &["-nd"]));
        assert!(!d("stash", &["list"]));
        // `stash create` is the rescue command the deny message recommends; it
        // writes a commit object without touching the tree.
        assert!(!d("stash", &["create"]));
        assert!(!d("restore", &["--staged", "."]));
        assert!(!d("status", &[]));
        assert!(!d("commit", &["-am", "x"]));
    }

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
}
