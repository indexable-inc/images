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

/// Shells whose `-c <script>` argument is itself a command line. Without this
/// the script stays one quoted token, so `bash -c 'git reset --hard'` never
/// presents `git` as a command word and the guard sees nothing (index#4211).
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh"];

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

/// What a statement actually runs: the command word and the tokens after it,
/// with env assignments and wrapper prefixes (`sudo`, `env`, ...) stepped over.
struct Invocation<'a> {
    head: &'a str,
    args: &'a [String],
}

fn invocation(stmt: &Statement) -> Option<Invocation<'_>> {
    let at = stmt
        .iter()
        .position(|w| !is_env_assignment(w) && !CMD_PREFIXES.contains(&w.as_str()))?;
    Some(Invocation {
        head: stmt[at].as_str(),
        args: &stmt[at + 1..],
    })
}

/// The script a `sh -c '<script>'` statement runs, if that is what it is.
fn shell_script(stmt: &Statement) -> Option<&str> {
    let call = invocation(stmt)?;
    // Match on the file name so `/bin/bash -c` and `/usr/bin/env sh -c` count.
    let name = std::path::Path::new(call.head).file_name()?.to_str()?;
    if !SHELLS.contains(&name) {
        return None;
    }
    // The token after the first `-c` is the script. Anything after that is `$0`
    // and the positional parameters, not code.
    let at = call
        .args
        .iter()
        .position(|a| a == "-c" || bundles_short(a, 'c'))?;
    call.args.get(at + 1).map(String::as_str)
}

/// `statements`, with every `sh -c '<script>'` replaced by the statements of
/// `<script>`. The recursion terminates because a script token is always
/// shorter than the command line it was parsed out of.
fn expanded_statements(cmd: &str) -> Vec<Statement> {
    let mut out = Vec::new();
    for stmt in statements(cmd) {
        match shell_script(&stmt) {
            Some(script) => out.extend(expanded_statements(script)),
            None => out.push(stmt),
        }
    }
    out
}

/// True when `arg` is a bundled short flag group (`-fd`, `-xfd`) containing
/// `short`.
fn bundles_short(arg: &str, short: char) -> bool {
    arg.starts_with('-')
        && !arg.starts_with("--")
        && arg.len() > 1
        && arg[1..].chars().all(char::is_alphanumeric)
        && arg[1..].contains(short)
}

/// True when any token is the long flag `long`, or a bundled short flag group
/// (`-fd`, `-xfd`) containing `short`. Scanning stops at `--`, after which
/// everything is a pathspec.
fn has_flag(args: &[String], short: char, long: &str) -> bool {
    args.iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| a == long || bundles_short(a, short))
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
        "reset"
            if args
                .iter()
                .any(|a| ["--hard", "--merge", "--keep"].contains(&a.as_str())) =>
        {
            Some("overwrites the working tree from a commit, discarding every uncommitted change")
        }
        // Branch-switching checkout refuses to clobber; the pathspec form
        // (`-- <path>`, or a bare `.`) and -f do not.
        "checkout" if flag('f', "--force") || args.iter().any(|a| a == "--" || a == ".") => Some(
            "restores the named paths from the index or a commit, discarding their uncommitted changes",
        ),
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
                args.iter()
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str),
                Some("list" | "show" | "create")
            ) =>
        {
            Some("moves uncommitted work off the working tree (or destroys already-stashed work)")
        }
        _ => None,
    }
}

/// Classify a git subcommand as one that writes to the repository, the index or
/// the working tree, naming what it changes. `None` means the call only reads.
///
/// This is the broad gate (index#4218). `discards_worktree` above asks the
/// narrow question "would this destroy someone's uncommitted work"; this one
/// asks "would this change a checkout that is not yours at all". A protected
/// primary checkout was found on a branch deleted upstream, 604 commits behind
/// main, with 534 files staged by nobody -- reached entirely through `git add`
/// and `git switch`, neither of which loses anything and neither of which the
/// narrow gate sees.
///
/// The set is closed and enumerated rather than "everything not known to be
/// read-only", to keep the guard's fail-open posture: a subcommand nobody has
/// classified must not turn into a refusal the first time someone runs it.
///
/// Three deliberate holes, all commands this guard's own denial text
/// recommends: `git stash list|show|create`, `git branch <name>` and `git
/// worktree add` are how a caller gets work out of a protected checkout, so
/// denying them would leave the refusal with no exit.
fn mutates_checkout(sub: &str, args: &[String]) -> Option<&'static str> {
    // git's universal "print what I would do"; every subcommand below that
    // accepts it honors it, and the ones that do not reject it and mutate
    // nothing either way.
    if args.iter().any(|a| a == "--dry-run") {
        return None;
    }
    // `-n` is only a dry run for the subcommands listed here: on `commit` it is
    // `--no-verify` and on `clean` it pairs with `-f`.
    let dry_n = || has_flag(args, 'n', "--dry-run");
    match sub {
        "add" => Some("stages changes into the index that every session here shares"),
        "am" => Some("applies a mailbox of patches onto the current branch"),
        // `--check`/`--stat`/`--numstat`/`--summary` only report on the patch.
        "apply"
            if !args
                .iter()
                .any(|a| ["--check", "--stat", "--numstat", "--summary"].contains(&a.as_str())) =>
        {
            Some("writes a patch into the working tree or the index")
        }
        "checkout" => Some("moves HEAD, or overwrites paths in the working tree"),
        "cherry-pick" => Some("commits another branch's change onto the current one"),
        "clean" if has_flag(args, 'f', "--force") && !dry_n() => Some("deletes untracked files"),
        "commit" => Some("writes a commit out of the index that every session here shares"),
        "merge" => Some("merges into the current branch"),
        "mv" if !dry_n() => Some("renames tracked paths"),
        "pull" => Some("fetches, then merges or rebases the current branch"),
        "rebase" => Some("rewrites the current branch"),
        "reset" => Some("moves HEAD, or rewrites the index"),
        "restore" => Some("restores paths over the index or the working tree"),
        "revert" => Some("commits the inverse of a change onto the current branch"),
        "rm" if !dry_n() => Some("deletes tracked paths"),
        "stash"
            if !matches!(
                args.iter()
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str),
                Some("list" | "show" | "create")
            ) =>
        {
            Some("moves uncommitted work off the working tree")
        }
        "switch" => Some("switches the checkout to another branch"),
        _ => None,
    }
}

/// The revision a pathspec-form `git checkout`/`git restore` reads from, when
/// that revision is something other than the checkout's own `HEAD`.
///
/// This is a different harm from losing uncommitted work. `git checkout <rev>
/// -- <path>` writes `<rev>` into the tree and leaves `HEAD` alone, so a shared
/// checkout stops matching its own `HEAD` for every session using it, and a
/// clean tree is no protection against that (index#4211).
fn reads_other_revision<'a>(sub: &str, args: &'a [String]) -> Option<&'a str> {
    let rev = match sub {
        "checkout" => {
            // Branch-creating and detaching forms take a start point; they move
            // HEAD to it rather than writing it over the tree behind HEAD's back.
            if args
                .iter()
                .any(|a| ["-b", "-B", "--orphan", "--detach"].contains(&a.as_str()))
            {
                return None;
            }
            let operands: Vec<&String> = args
                .iter()
                .take_while(|a| a.as_str() != "--")
                .filter(|a| !a.starts_with('-'))
                .collect();
            // A tree-ish is only present when a pathspec follows it, either
            // after `--` or as a further operand. A lone operand is a branch to
            // switch to or a path to restore from the index, and neither reads
            // another revision.
            let pathspec_follows = args.iter().any(|a| a == "--") || operands.len() > 1;
            match operands.first() {
                Some(first) if pathspec_follows => first.as_str(),
                _ => return None,
            }
        }
        // `restore` names its source explicitly, so there is nothing to
        // disambiguate. Without `--source` it reads the index, not a revision.
        "restore" => {
            let mut it = args.iter();
            let mut source = None;
            while let Some(a) = it.next() {
                if let Some(v) = a.strip_prefix("--source=") {
                    source = Some(v);
                    break;
                }
                if a == "--source" || a == "-s" {
                    source = it.next().map(String::as_str);
                    break;
                }
            }
            source?
        }
        _ => return None,
    };
    // Restoring from HEAD leaves the tree and HEAD agreeing, which is the whole
    // point of the check.
    (rev != "HEAD" && rev != "@").then_some(rev)
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
        .and_then(|re| {
            re.captures(url.trim())
                .map(|c| format!("{}/{}", &c[1], &c[2]))
        })
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

/// Run `git -C <top> --no-optional-locks <args>` and return stdout on a clean
/// exit. `None` on any failure, which every caller reads as "cannot tell".
///
/// `--no-optional-locks` because this runs against a checkout other sessions
/// are using: a plain `git status` takes `.git/index.lock` and rewrites the
/// index to refresh its stat cache, which can lose a race with someone else's
/// `git add`.
fn git_stdout(git: &str, top: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(git)
        .args(["-C", top, "--no-optional-locks"])
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(())?;
    String::from_utf8(out.stdout).ok()
}

/// The refs the deny message itself prescribes: a `rescue/*` branch cut from
/// `git stash create`.
///
/// Deliberately not every ref in the checkout. A path the working tree has
/// deleted is absent from every ref that never had it, so scanning everything
/// names an ancient unrelated branch as the rescue snapshot, which is worse
/// than saying nothing. `refs/stash` is out for its own reasons: a successful
/// `git stash push` leaves the tree clean, so a stash can only be stale by the
/// time this runs, and its short name `stash` means `stash@{0}`, which moves.
const SNAPSHOT_REF_PATTERN: &str = "refs/heads/rescue";
/// Rescue refs compared before giving up, newest first.
///
/// The message prescribes one branch per incident and nothing prunes them, so
/// this namespace grows without bound. `git-guard` has a 10s hook timeout and a
/// timed-out `PreToolUse` hook emits no deny at all, so an accumulated history
/// must not be able to walk the check into failing open.
const SNAPSHOT_CANDIDATE_LIMIT: usize = 4;

/// The status codes `git status --porcelain` uses for an unmerged path.
const UNMERGED: [&str; 7] = ["DD", "AU", "UD", "UA", "DU", "AA", "UU"];

/// What a subcommand destroys beyond tracked content, and therefore what a
/// snapshot has to cover before the message may promise a way back.
///
/// Scoping this per subcommand is the difference between a working feature and
/// a dead one. Every protected checkout has ignored content in it (74 entries
/// in `~/.config/nix` as this was written), so treating ignored files as
/// disqualifying for every command means the snapshot wording never appears at
/// all. `git reset --hard`, `git checkout`, `git restore`, `git switch` and a
/// plain `git stash push` do not touch untracked or ignored files.
#[derive(Clone, Copy)]
struct Destroys {
    untracked: bool,
    ignored: bool,
}

fn destroys_beyond_tracked(sub: &str, args: &[String]) -> Destroys {
    // `git clean`'s `-x` and `-X` have no long form, so they are matched on the
    // short flag alone rather than through `has_flag`.
    let short = |c: char| {
        args.iter()
            .take_while(|a| a.as_str() != "--")
            .any(|a| bundles_short(a, c))
    };
    match sub {
        // `git clean -f` deletes untracked files; `-x` and `-X` extend it to
        // ignored ones.
        "clean" => Destroys {
            untracked: true,
            ignored: short('x') || short('X'),
        },
        // `stash push -u` takes untracked files with it, `-a` ignored ones too.
        "stash" => Destroys {
            untracked: has_flag(args, 'u', "--include-untracked") || has_flag(args, 'a', "--all"),
            ignored: has_flag(args, 'a', "--all"),
        },
        _ => Destroys {
            untracked: false,
            ignored: false,
        },
    }
}

/// A ref that already holds this working tree exactly as it stands, if there
/// is one.
///
/// `git stash create` writes such a commit and the deny message tells the
/// operator to point a `rescue/*` branch at it. Once they have, "there is no
/// blob in the object database and no way back" is false, and the message read
/// as if the snapshot were a precondition that should have unlocked the command
/// (ix#8785).
///
/// Read-only against the shared checkout: no object is written, no ref moves,
/// and the index `git diff` refreshes is a private copy. It runs only on the
/// refusal path.
fn snapshot_ref(git: &str, top: &str, common: &str, destroys: Destroys) -> Option<String> {
    if !diff_covers_working_tree(git, top, destroys) {
        return None;
    }
    // Sorted here rather than with `--sort=-committerdate`, which reads every
    // ref's object and so fails the whole call on one ref with a missing
    // object, silently taking every good rescue ref with it. The prescribed
    // name is `rescue/$(date +%Y-%m-%d-%H%M)`, so reverse lexicographic order
    // is newest first.
    let refs = git_stdout(
        git,
        top,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            SNAPSHOT_REF_PATTERN,
        ],
    )?;
    let mut candidates: Vec<&str> = refs.lines().filter(|r| shell_safe_ref(r)).collect();
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    candidates.truncate(SNAPSHOT_CANDIDATE_LIMIT);
    if candidates.is_empty() {
        return None;
    }
    let index = scratch_index(common)?;
    candidates
        .into_iter()
        .find(|rev| holds_working_tree(git, top, index.path(), rev))
        .map(str::to_owned)
}

/// Whether `git diff <rev>` looks at everything this command would destroy.
///
/// A snapshot cannot be said to hold content that no comparison looks at.
/// `git stash create` captures neither untracked nor ignored files, and `git
/// diff` skips those as well as any path flagged assume-unchanged or
/// skip-worktree. Where the command about to run would delete such a path,
/// `diff --quiet` comes back clean and the message promises a way back for an
/// ignored build tree that is in no commit, so those cases are refused instead.
///
/// A sparse checkout marks every out-of-cone path skip-worktree, so this
/// returns false there and the message keeps its unrecoverable wording.
///
/// Anything unreadable is a no: this decides whether to make a promise.
fn diff_covers_working_tree(git: &str, top: &str, destroys: Destroys) -> bool {
    // Spelled out rather than left to the repo: `status.showUntrackedFiles =
    // no` hides exactly the files this has to see, and
    // `submodule.<name>.ignore` hides dirty submodule content from both the
    // status and the diff below, which is the one way the two can agree while
    // content is still lost. `traditional` and `normal` collapse whole
    // directories, which is all it takes to answer "is there any", without
    // enumerating a build tree.
    let untracked = if destroys.untracked {
        "--untracked-files=normal"
    } else {
        "--untracked-files=no"
    };
    let ignored = if destroys.ignored {
        "--ignored=traditional"
    } else {
        "--ignored=no"
    };
    let Some(status) = git_stdout(
        git,
        top,
        &[
            "status",
            "--porcelain",
            "--ignore-submodules=none",
            untracked,
            ignored,
        ],
    ) else {
        return false;
    };
    let uncovered = status.lines().any(|line| {
        let xy = line.get(..2).unwrap_or_default();
        xy == "??" || xy == "!!" || UNMERGED.contains(&xy)
    });
    if uncovered {
        return false;
    }
    // `ls-files -v` tags skip-worktree `S` and assume-unchanged with a
    // lowercase letter. Both make a path invisible to `git diff` while leaving
    // it in reach of `git reset --hard`.
    let Some(files) = git_stdout(git, top, &["ls-files", "-v"]) else {
        return false;
    };
    !files
        .lines()
        .any(|l| l.starts_with(|c: char| c == 'S' || c.is_ascii_lowercase()))
}

/// Whether `rev` holds the working tree exactly as it stands.
///
/// `git diff` and not a comparison of blob ids, because a tree entry carries a
/// mode that a blob does not. `chmod +x` and a symlink replaced by a regular
/// file of the same text both leave every blob id untouched, and both are real
/// losses. Comparing the whole tree also means a path deleted in the working
/// tree cannot match a ref that simply never had it.
fn holds_working_tree(git: &str, top: &str, index: &std::path::Path, rev: &str) -> bool {
    std::process::Command::new(git)
        .args([
            "-C",
            top,
            "--no-optional-locks",
            "diff",
            "--quiet",
            // `diff.ignoreSubmodules` would otherwise hide dirty submodule
            // content that `git stash create` never captured.
            "--ignore-submodules=none",
            rev,
            "--",
        ])
        .env("GIT_INDEX_FILE", index)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.code() == Some(0))
}

/// A private copy of the index for `git diff` to refresh.
///
/// `git diff` rewrites the index stat cache even under `--no-optional-locks`,
/// and this checkout belongs to every session on the machine, so it gets a
/// copy rather than the real one.
fn scratch_index(common: &str) -> Option<tempfile::NamedTempFile> {
    // Written, not `fs::copy`d: copy carries the source's mode over the 0600
    // the temp file was created with, and an index names every tracked path in
    // a checkout whose whole premise is that the machine has other principals
    // on it.
    let bytes = std::fs::read(std::path::Path::new(common).join("index")).ok()?;
    let mut scratch = tempfile::NamedTempFile::new().ok()?;
    std::io::Write::write_all(&mut scratch, &bytes).ok()?;
    std::io::Write::flush(&mut scratch).ok()?;
    Some(scratch)
}

/// Whether a refname is safe to paste into the command the message tells the
/// operator to run.
///
/// `git branch` accepts `rescue/$(id)`, every session on the machine shares
/// this checkout and can create one, and a fetch can bring one in. This message
/// exists to be run, so a name outside this set falls back to the no-snapshot
/// wording rather than becoming a command injection.
fn shell_safe_ref(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
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
    /// A ref that already holds this exact content, from `snapshot_ref`.
    snapshot: Option<&'a str>,
}

fn deny_message(refusal: &Refusal<'_>) -> String {
    let Refusal {
        git,
        top,
        sub,
        effect,
        dirty,
        snapshot,
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
    let head = format!(
        "Refusing `git {sub}` in {top}.\n\n\
         That checkout is shared by every agent session on this machine, it currently \
         holds {count} entries of uncommitted work, and `git {sub}` {effect}."
    );
    let worktree = format!(
        "Do the work in a worktree of your own, where destructive commands are yours to \
         run:\n\
         \x20   git -C {top} worktree add /tmp/worktree/{slug}/<name> -b <branch> {start}\n\
         \x20   git -C /tmp/worktree/{slug}/<name> submodule update --init --recursive",
        start = snapshot.unwrap_or("origin/main"),
    );
    // With a snapshot in hand the old text is false, and it read as if taking
    // the snapshot were the precondition that unlocks the command (ix#8785).
    if let Some(rescue) = snapshot {
        return format!(
            "{head} {rescue} already holds this working tree exactly as it stands, so there \
             is a way back. The \
             refusal is not about recoverability, it is policy: that working tree is every \
             session's, so a snapshot does not make it yours to move.\n\n\
             Held by {rescue}:\n{shown}{more}\n\n\
             {worktree}\n\n\
             That worktree opens with the content above already committed, as the WIP \
             commit `git stash create` wrote, so nothing there needs `git {sub}`. To carry \
             on from it as uncommitted work:\n\
             \x20   git -C /tmp/worktree/{slug}/<name> reset --soft HEAD^\n\n\
             (git-guard hook, ENG-9964, ix#8785)"
        );
    }
    // A staged blob is in the object database whatever else is true, so the
    // original sentence was its own small version of this bug.
    let staged = dirty
        .iter()
        .any(|l| l.starts_with(|c: char| !matches!(c, ' ' | '?')));
    let loss = if staged {
        "Some of it is staged, so those blobs are in the object database, but no snapshot \
         holds this working tree as it stands"
    } else {
        "None of it is staged, so there is no blob in the object database and no way back"
    };
    format!(
        "{head} {loss} (ENG-9964: exactly this command destroyed 7 uncommitted lines \
         belonging to another session, recovered only because a nix eval had happened to \
         copy the dirty tree into the store).\n\n\
         Would be lost:\n{shown}{more}\n\n\
         Snapshot it first, which does not touch the working tree:\n\
         \x20   git -C {top} branch rescue/$(date +%Y-%m-%d-%H%M) \"$(git -C {top} stash create)\"\n\n\
         The snapshot does not unlock `git {sub}` here. It saves the work where it stands, \
         and gives the worktree below something to start from.\n\n\
         {worktree}\n\n\
         (git-guard hook, ENG-9964)"
    )
}

/// The refusal for reading another revision into a shared tree. Separate from
/// `deny_message` because nothing is being lost here; the harm is that everyone
/// else's tree stops matching their own HEAD.
fn desync_message(git: &str, top: &str, sub: &str, rev: &str) -> String {
    let slug = origin_slug(git, top);
    format!(
        "Refusing `git {sub} {rev}` in {top}.\n\n\
         That checkout is shared by every agent session on this machine. `git {sub}` with a \
         tree-ish other than HEAD writes {rev} over the working tree and leaves HEAD where it \
         is, so every other session finds a staged diff the size of the gap between {rev} and \
         HEAD, which reads like a merge that went wrong. A clean tree is no protection: there \
         is nothing to lose and the desync happens anyway (index#4211: one such call staged \
         534 files in this checkout and cost an operator session to diagnose).\n\n\
         Read {rev} without moving anyone's tree:\n\
         \x20   git -C {top} grep -n <pattern> {rev}\n\
         \x20   git -C {top} show {rev}:<path>\n\n\
         Or get a tree of your own at that revision:\n\
         \x20   git -C {top} worktree add /tmp/worktree/{slug}/<name> -b <branch> {rev}\n\
         \x20   git -C /tmp/worktree/{slug}/<name> submodule update --init --recursive\n\n\
         (git-guard hook, index#4211)"
    )
}

/// The refusal for mutating a protected primary checkout at all. Separate from
/// `deny_message` and `desync_message` because nothing is necessarily being
/// lost and no foreign revision is being read: the harm is that a checkout
/// every session on the machine shares stops being the tree they all assume
/// (index#4218).
fn mutation_message(git: &str, top: &str, sub: &str, effect: &str) -> String {
    let slug = origin_slug(git, top);
    format!(
        "Refusing `git {sub}` in {top}.\n\n\
         {top} is a protected primary checkout -- shared by every agent session on this \
         machine, and not yours to change. `git {sub}` {effect}. Never work in a primary \
         checkout, not even when nothing would be lost: this one was found on a branch that \
         had been deleted upstream, 604 commits behind main, with 534 files staged by nobody. \
         No edit tool was involved. `git add` and `git switch` through Bash did all of it \
         (index#4218, index#4216).\n\n\
         Do the work in a worktree of your own, where every git command is yours to run:\n\
         \x20   git -C {top} worktree add /tmp/worktree/{slug}/<name> -b <branch> origin/main\n\
         \x20   git -C /tmp/worktree/{slug}/<name> submodule update --init --recursive\n\n\
         Reading here is always fine: status, log, diff, show, grep, ls-files, rev-parse, \
         branch, fetch, worktree list/add, and anything with --dry-run all still run.\n\n\
         (git-guard hook, index#4218; kill switch CLAUDE_CODE_DISABLE_GIT_GUARD=1)"
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
    let effect = discards_worktree(&call.sub, &call.args);
    let other_rev = reads_other_revision(&call.sub, &call.args);
    let mutation = mutates_checkout(&call.sub, &call.args);
    if effect.is_none() && other_rev.is_none() && mutation.is_none() {
        return None;
    }
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

    // `--no-optional-locks`: a plain `git status` rewrites the index to refresh
    // its stat cache, and this checkout belongs to every session on the machine.
    let status = std::process::Command::new(git)
        .args(["-C", &top, "--no-optional-locks", "status", "--porcelain"])
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
    // Uncommitted work that this command would destroy is the first thing to
    // say, because it is the only unrecoverable one.
    if let Some(effect) = effect.filter(|_| !dirty.is_empty()) {
        // Only on this branch, so the snapshot lookup never runs on a call the
        // guard is about to allow.
        let snapshot = snapshot_ref(git, &top, &common, destroys_beyond_tracked(sub, &call.args));
        return Some(deny_message(&Refusal {
            git,
            top: &top,
            sub,
            effect,
            dirty: &dirty,
            snapshot: snapshot.as_deref(),
        }));
    }
    // A clean tree has nothing to destroy, but reading another revision into it
    // still leaves every session out of sync with its own HEAD.
    if let Some(rev) = other_rev {
        return Some(desync_message(git, &top, sub, rev));
    }
    // Neither of the specific harms applies, so this is the broad rule: the
    // checkout is not yours and the command writes to it (index#4218).
    mutation.map(|effect| mutation_message(git, &top, sub, effect))
}

/// Everything `git_guard` reads from the process environment, so that the
/// decision below is a pure function of its inputs and the tests can drive it
/// without a real hook invocation and without mutating the environment.
struct GitGuardEnv {
    /// Kill switch `CLAUDE_CODE_DISABLE_GIT_GUARD`.
    disabled: bool,
    /// The `git` to shell out to: `IX_GIT` from the wrapper, else PATH.
    git: String,
    /// The `primaryCheckouts` globs. Empty means the guard is not installed.
    protected: Vec<String>,
    /// Where a statement runs when the payload carries no `cwd`.
    fallback_cwd: std::path::PathBuf,
}

impl GitGuardEnv {
    fn read() -> Self {
        Self {
            disabled: crate::flag_set("CLAUDE_CODE_DISABLE_GIT_GUARD"),
            git: std::env::var("IX_GIT").unwrap_or_else(|_| "git".to_owned()),
            protected: crate::primary_checkouts(),
            fallback_cwd: std::path::PathBuf::from(
                std::env::var("PWD").unwrap_or_else(|_| ".".to_owned()),
            ),
        }
    }
}

/// The whole decision: `Some(reason)` refuses the Bash call, `None` allows it.
///
/// Every early return here is a fail-open: kill switch, non-Bash tool, no
/// protected list, a payload shape that carries no command. Only `judge` ever
/// refuses.
fn git_guard_decision(env: &GitGuardEnv, payload: &Value) -> Option<String> {
    if env.disabled || env.protected.is_empty() {
        return None;
    }
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return None;
    }
    let mut cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map_or_else(|| env.fallback_cwd.clone(), std::path::PathBuf::from);

    for stmt in expanded_statements(&command_of(payload)) {
        let Some(run) = invocation(&stmt) else {
            continue;
        };
        // Track `cd` across the chain: `cd <primary> && git reset --hard` must
        // be judged against the cd target, not the payload cwd.
        if run.head == "cd" {
            if let Some(target) = run.args.iter().find(|a| !a.starts_with('-')) {
                cwd = resolve(&cwd, target);
            }
            continue;
        }
        if run.head != "git" {
            continue;
        }
        let Some(call) = parse_git_call(run.args, &cwd) else {
            continue;
        };
        if let Some(reason) = judge(&env.git, &env.protected, &call) {
            return Some(reason);
        }
    }
    None
}

/// `PreToolUse(Bash)`: refuse a git command that would mutate a shared primary
/// checkout -- by destroying uncommitted work there (ENG-9964), by leaving the
/// tree out of sync with its own HEAD (index#4211), or simply by writing to a
/// checkout that belongs to every session and to none (index#4218).
///
/// Why a hook and not a git hook: git has no `pre-reset`, `pre-checkout`,
/// `pre-clean` or `pre-stash`, `post-checkout` runs after the damage, and
/// `reference-transaction` never fires for `git clean` or for a `reset --hard`
/// that does not move HEAD. `PreToolUse` is the only seam that sees the command
/// before it runs (ENG-9964).
///
/// Why here and not in `worktree-guard`: that guard judges the target path of
/// an edit tool and never sees Bash, so `git add` and `git switch` walked
/// straight past it (index#4218).
pub fn git_guard() {
    let env = GitGuardEnv::read();
    if env.disabled {
        return;
    }
    let Some(payload) = payload() else { return };
    if let Some(reason) = git_guard_decision(&env, &payload) {
        deny(reason);
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
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use serde_json::{Value, json};

    use super::{
        GitGuardEnv, discards_worktree, expanded_statements, git_guard_decision, grep_walks_tree,
        has_flag, is_recursive_flag, mutates_checkout, parse_git_call, reads_other_revision,
        statements,
    };

    fn toks(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Skip the repo-backed tests where `git` is absent (minimal sandboxes),
    /// so the suite stays green there rather than failing on a missing tool.
    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    /// `GIT_CONFIG_{GLOBAL,SYSTEM}` are pinned off so the fixture does not
    /// inherit a developer's hooks path, signing key or default branch.
    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} in {}", dir.display());
    }

    /// A throwaway primary checkout with one commit, under a canonicalized
    /// root: git's `--show-toplevel` resolves symlinks and macOS `TMPDIR` is
    /// one, so an unresolved path would never match a protected glob.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        primary: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical tempdir");
        let primary = root.join("primary");
        std::fs::create_dir(&primary).expect("mkdir primary");
        run_git(&primary, &["init", "--quiet"]);
        run_git(&primary, &["config", "user.email", "guard@example.com"]);
        run_git(&primary, &["config", "user.name", "guard"]);
        std::fs::write(primary.join("README"), "seed\n").expect("write README");
        run_git(&primary, &["add", "README"]);
        run_git(&primary, &["commit", "--quiet", "-m", "seed"]);
        Fixture {
            _dir: dir,
            root,
            primary,
        }
    }

    fn guard_env(protected: &[&str]) -> GitGuardEnv {
        GitGuardEnv {
            disabled: false,
            git: "git".to_owned(),
            protected: protected.iter().map(|s| (*s).to_owned()).collect(),
            // Distinct from any fixture path: a test that accidentally relied
            // on the fallback must not silently land inside a protected glob.
            fallback_cwd: PathBuf::from("/nonexistent/fallback"),
        }
    }

    /// A fixture checkout plus the environment that protects it, or `None`
    /// where `git` is absent so the repo-backed tests skip rather than fail.
    ///
    /// Every test below opens with this, so the skip and the protected glob
    /// cannot drift apart between them.
    /// A fixture checkout and the environment that protects it.
    struct Protected {
        fx: Fixture,
        env: GitGuardEnv,
    }

    fn protected_fixture() -> Option<Protected> {
        if !git_available() {
            return None;
        }
        let fx = fixture();
        let env = guard_env(&[fx.primary.to_str().expect("utf8")]);
        Some(Protected { fx, env })
    }

    fn bash(cwd: &Path, command: &str) -> Value {
        json!({
            "tool_name": "Bash",
            "cwd": cwd.to_str().expect("utf8 cwd"),
            "tool_input": {"command": command},
        })
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
    fn shell_wrappers_are_unwrapped() {
        // index#4211: the script stayed one token, so no statement ever had
        // `git` as its command word and the guard saw nothing.
        assert_eq!(
            expanded_statements("bash -c 'cd /a && git checkout -q origin/main -- .'"),
            vec![
                toks(&["cd", "/a"]),
                toks(&["git", "checkout", "-q", "origin/main", "--", "."])
            ]
        );
        // sh, zsh, an absolute path, a wrapper prefix, and bundled `-ec` all
        // reach the same place.
        for cmd in [
            r#"sh -c "git clean -fd""#,
            "zsh -c 'git clean -fd'",
            "/bin/bash -c 'git clean -fd'",
            "sudo sh -c 'git clean -fd'",
            "bash -ec 'git clean -fd'",
            // Nesting: one shell inside another.
            r#"bash -c "sh -c 'git clean -fd'""#,
        ] {
            assert_eq!(
                expanded_statements(cmd),
                vec![toks(&["git", "clean", "-fd"])],
                "{cmd}"
            );
        }
        // Tokens after the script are $0 and the positional parameters, so they
        // are not code and must not be re-parsed.
        assert_eq!(
            expanded_statements("bash -c 'git status' bash git clean -fd"),
            vec![toks(&["git", "status"])]
        );
        // A shell that is not running an inline script is left alone.
        assert_eq!(
            expanded_statements("bash ./script.sh"),
            vec![toks(&["bash", "./script.sh"])]
        );
    }

    #[test]
    fn pathspec_checkout_from_another_revision() {
        let r = |sub: &str, a: &[&str]| reads_other_revision(sub, &toks(a)).map(str::to_owned);
        // index#4211: the exact shape that desynced the shared checkout.
        assert_eq!(
            r("checkout", &["-q", "origin/main", "--", "."]),
            Some("origin/main".to_owned())
        );
        // Without `--`, a second operand is still a pathspec.
        assert_eq!(
            r("checkout", &["origin/main", "src"]),
            Some("origin/main".to_owned())
        );
        assert_eq!(
            r("restore", &["--source=origin/main", "."]),
            Some("origin/main".to_owned())
        );
        assert_eq!(
            r("restore", &["-s", "HEAD~1", "."]),
            Some("HEAD~1".to_owned())
        );
        // HEAD is the tree's own revision, so it cannot desync anything.
        assert_eq!(r("checkout", &["HEAD", "--", "."]), None);
        assert_eq!(r("restore", &["--source", "@", "."]), None);
        // Forms that read the index, move HEAD, or create a branch.
        assert_eq!(r("checkout", &["--", "."]), None);
        assert_eq!(r("checkout", &["."]), None);
        assert_eq!(r("checkout", &["main"]), None);
        assert_eq!(r("checkout", &["-b", "feature", "origin/main"]), None);
        assert_eq!(r("checkout", &["--detach", "origin/main"]), None);
        assert_eq!(r("restore", &["."]), None);
        assert_eq!(r("reset", &["--hard", "origin/main"]), None);
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
    fn mutating_git_classification() {
        let m = |sub: &str, a: &[&str]| mutates_checkout(sub, &toks(a)).is_some();
        // index#4218: the two that reached the shared checkout, plus the rest
        // of the enumerated set.
        assert!(m("add", &["-A"]));
        assert!(m("switch", &["main"]));
        for sub in [
            "am",
            "apply",
            "checkout",
            "cherry-pick",
            "commit",
            "merge",
            "mv",
            "pull",
            "rebase",
            "reset",
            "restore",
            "revert",
            "rm",
            "stash",
        ] {
            assert!(m(sub, &[]), "{sub}");
        }
        assert!(m("clean", &["-fd"]));
        // Read-only git in a protected checkout has to keep working, or the
        // guard is switched off within the hour.
        for sub in [
            "status",
            "log",
            "diff",
            "show",
            "rev-parse",
            "ls-files",
            "branch",
            "worktree",
            "fetch",
            "grep",
            "blame",
            "describe",
            "remote",
            "push",
            "tag",
        ] {
            assert!(!m(sub, &[]), "{sub}");
        }
        // `--dry-run` is git's universal "print what I would do".
        assert!(!m("add", &["--dry-run", "."]));
        assert!(!m("commit", &["--dry-run"]));
        assert!(!m("rm", &["-n", "README"]));
        assert!(!m("mv", &["-n", "a", "b"]));
        assert!(!m("clean", &["-nd"]));
        assert!(!m("clean", &[]));
        // `commit -n` is --no-verify, not a dry run; it must still be denied.
        assert!(m("commit", &["-n", "-m", "x"]));
        // `apply` in its reporting forms writes nothing.
        assert!(!m("apply", &["--check", "p.patch"]));
        assert!(!m("apply", &["--stat", "p.patch"]));
        // The escape hatches the denial texts recommend must survive the guard,
        // or the advice is a dead end.
        assert!(!m("stash", &["create"]));
        assert!(!m("stash", &["list"]));
        assert!(!m("stash", &["show"]));
        assert!(!m("branch", &["rescue/2026-07-27"]));
        assert!(!m(
            "worktree",
            &["add", "/tmp/worktree/o/r/n", "-b", "b", "origin/main"]
        ));
    }

    #[test]
    fn mutating_git_in_a_protected_checkout_is_denied() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        let reason = git_guard_decision(&env, &bash(&fx.primary, "git add -A"))
            .expect("git add in a protected primary checkout is denied");
        // The message has to name the path and the way out, or the agent
        // cannot act on it.
        assert!(
            reason.contains(fx.primary.to_str().expect("utf8")),
            "{reason}"
        );
        assert!(reason.contains("worktree add /tmp/worktree/"), "{reason}");
        // The tree is clean and nothing here loses work: this is exactly the
        // hole the narrow ENG-9964 gate left open.
        for cmd in [
            "git add -A",
            "git commit -m wip",
            "git switch -c topic",
            "git checkout -b topic",
            "git restore --staged .",
            "git reset --soft HEAD~1",
            "git merge origin/main",
            "git rebase origin/main",
            "git cherry-pick deadbeef",
            "git revert HEAD",
            "git apply /tmp/p.patch",
            "git rm README",
            "git mv README R2",
            "git pull",
        ] {
            assert!(
                git_guard_decision(&env, &bash(&fx.primary, cmd)).is_some(),
                "{cmd}"
            );
        }
        // The evasions the parser already handles must reach the new rule too.
        assert!(
            git_guard_decision(&env, &bash(&fx.root, "cd primary && git add -A")).is_some(),
            "cd into the checkout"
        );
        let dashc = format!("git -C {} add -A", fx.primary.display());
        assert!(
            git_guard_decision(&env, &bash(&fx.root, &dashc)).is_some(),
            "-C into the checkout"
        );
        let wrapped = format!("bash -c 'git -C {} commit -am wip'", fx.primary.display());
        assert!(
            git_guard_decision(&env, &bash(&fx.root, &wrapped)).is_some(),
            "buried in bash -c"
        );
    }

    /// `git -C <dir> stash create`, the exact rescue command the deny message
    /// prescribes, returning the commit it wrote.
    fn stash_create(dir: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["stash", "create"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git stash create");
        assert!(out.status.success(), "git stash create");
        String::from_utf8(out.stdout)
            .expect("utf8 sha")
            .trim()
            .to_owned()
    }

    /// The one uncommitted edit these tests are about. It is what puts the
    /// checkout on the refusal path where the snapshot is looked for at all.
    fn dirty_readme(dir: &Path) {
        std::fs::write(dir.join("README"), "seed\nedited\n").expect("dirty README");
    }

    /// Point `name` at a snapshot of `dir` exactly as it stands: the `git
    /// branch rescue/... "$(git stash create)"` line the deny message prints.
    fn rescue_branch(dir: &Path, name: &str) {
        run_git(dir, &["branch", name, &stash_create(dir)]);
    }

    /// The state the message describes once the operator has taken the snapshot
    /// it prescribes: a dirty protected checkout, held by `rescue/test`.
    ///
    /// Tests that need committed content inside the snapshot (a `.gitignore`, a
    /// submodule) build it from `protected_fixture` and the two helpers above
    /// instead, so the setup lands before the snapshot is cut.
    fn snapshotted() -> Option<Protected> {
        let Protected { fx, env } = protected_fixture()?;
        dirty_readme(&fx.primary);
        rescue_branch(&fx.primary, "rescue/test");
        Some(Protected { fx, env })
    }

    /// ix#8785: the message prescribed a rescue snapshot and then kept saying
    /// "no blob in the object database and no way back" after the operator had
    /// taken it, which reads as if the snapshot should have unlocked the
    /// command.
    #[test]
    fn a_rescue_snapshot_changes_the_refusal_from_loss_to_policy() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        dirty_readme(&fx.primary);

        let before = git_guard_decision(&env, &bash(&fx.primary, "git stash push -m wip"))
            .expect("a dirty shared checkout is denied");
        assert!(before.contains("no way back"), "{before}");
        assert!(before.contains("Would be lost:"), "{before}");
        assert!(
            before.contains("The snapshot does not unlock `git stash` here."),
            "{before}"
        );

        rescue_branch(&fx.primary, "rescue/test");

        let after = git_guard_decision(&env, &bash(&fx.primary, "git stash push -m wip"))
            .expect("the refusal stands: this is policy, not recoverability");
        assert!(!after.contains("no way back"), "{after}");
        assert!(!after.contains("Would be lost:"), "{after}");
        assert!(after.contains("Held by rescue/test:"), "{after}");
        assert!(
            after.contains("rescue/test already holds this working tree exactly as it stands"),
            "{after}"
        );
        // The way forward has to name the snapshot, or it is the same dead end.
        assert!(after.contains("-b <branch> rescue/test"), "{after}");

        // A ref that no longer matches the working tree does not count.
        std::fs::write(fx.primary.join("README"), "seed\nedited again\n").expect("re-dirty");
        let moved = git_guard_decision(&env, &bash(&fx.primary, "git stash push -m wip"))
            .expect("still denied");
        assert!(moved.contains("no way back"), "{moved}");
    }

    /// Everything below pins the direction that matters: claiming a snapshot
    /// that does not cover the work is the dangerous way to be wrong, while
    /// missing one only restores the old behaviour.
    ///
    /// A mode change is a real loss and no blob carries it, so an older rescue
    /// ref does not hold it. Comparing blob ids said it did.
    #[test]
    fn a_mode_change_is_not_held_by_a_snapshot_taken_before_it() {
        let Some(Protected { fx, env }) = snapshotted() else {
            return;
        };
        let mode = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(fx.primary.join("README"), mode).expect("chmod +x");

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git reset --hard"))
            .expect("a dirty shared checkout is denied");
        assert!(reason.contains("no way back"), "{reason}");
    }

    /// A path the working tree has deleted is absent from every ref that never
    /// had it, so scanning all refs matched an arbitrary ancient branch. Only
    /// the refs the message prescribes are consulted.
    #[test]
    fn an_unrelated_branch_is_never_named_as_the_snapshot() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        // A branch from before the file existed, exactly the vacuous match.
        run_git(&fx.primary, &["branch", "ancient"]);
        std::fs::write(fx.primary.join("LATER"), "later\n").expect("write LATER");
        run_git(&fx.primary, &["add", "LATER"]);
        run_git(&fx.primary, &["commit", "--quiet", "-m", "later"]);
        std::fs::remove_file(fx.primary.join("LATER")).expect("delete LATER");

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git reset --hard"))
            .expect("a dirty shared checkout is denied");
        assert!(reason.contains("no way back"), "{reason}");
        assert!(!reason.contains("ancient"), "{reason}");
    }

    /// The message is written to be run. A refname carrying shell syntax is not
    /// pasted into it, however well it matches: any session sharing the
    /// checkout can create one, and a fetch can bring one in.
    #[test]
    fn a_refname_carrying_shell_syntax_is_not_pasted_into_the_message() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        dirty_readme(&fx.primary);
        let snap = stash_create(&fx.primary);
        // Sorts ahead of the safe one, so the test pins that filtering happens
        // before the candidate cap rather than by luck of ordering.
        run_git(&fx.primary, &["branch", "rescue/a$(id)", &snap]);

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git reset --hard"))
            .expect("a dirty shared checkout is denied");
        assert!(!reason.contains("$(id)"), "{reason}");
        assert!(reason.contains("no way back"), "{reason}");

        // A safe name alongside it is still found.
        run_git(&fx.primary, &["branch", "rescue/safe", &snap]);
        let ok =
            git_guard_decision(&env, &bash(&fx.primary, "git reset --hard")).expect("still denied");
        assert!(ok.contains("Held by rescue/safe:"), "{ok}");
        assert!(!ok.contains("$(id)"), "{ok}");
    }

    /// An untracked file is not in a `git stash create` snapshot and `git diff`
    /// does not look at it, so the loss wording is the truthful one.
    #[test]
    fn an_untracked_file_is_never_reported_as_snapshotted() {
        let Some(Protected { fx, env }) = snapshotted() else {
            return;
        };
        std::fs::write(fx.primary.join("NEW"), "not in the snapshot\n").expect("untracked file");

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git clean -fd"))
            .expect("a dirty shared checkout is denied");
        assert!(reason.contains("no way back"), "{reason}");
    }

    /// `git clean -fdx` deletes ignored files, which no `git stash create`
    /// snapshot holds, so the loss wording is the truthful one there.
    ///
    /// `git reset --hard` does not touch them, and every protected checkout has
    /// ignored content in it, so disqualifying on ignored files for every
    /// subcommand made the whole feature inert in production. Both halves are
    /// pinned here.
    #[test]
    fn ignored_files_only_disqualify_the_commands_that_delete_them() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        std::fs::write(fx.primary.join(".gitignore"), "build/\n").expect("write .gitignore");
        run_git(&fx.primary, &["add", ".gitignore"]);
        run_git(&fx.primary, &["commit", "--quiet", "-m", "ignore build"]);
        dirty_readme(&fx.primary);
        rescue_branch(&fx.primary, "rescue/test");
        std::fs::create_dir(fx.primary.join("build")).expect("mkdir build");
        std::fs::write(fx.primary.join("build/out"), "artifact\n").expect("write artifact");

        let cleaned = git_guard_decision(&env, &bash(&fx.primary, "git clean -fdx"))
            .expect("a dirty shared checkout is denied");
        assert!(cleaned.contains("no way back"), "{cleaned}");
        assert!(!cleaned.contains("rescue/test"), "{cleaned}");

        // An ignored build tree is none of these commands' business, and
        // without -x that includes `git clean` itself.
        for cmd in [
            "git clean -fd",
            "git reset --hard",
            "git checkout .",
            "git stash push -m wip",
        ] {
            let reason = git_guard_decision(&env, &bash(&fx.primary, cmd)).expect("denied");
            assert!(reason.contains("Held by rescue/test:"), "{cmd}\n{reason}");
        }

        // An untracked file flips `git clean -fd`, which deletes it, but not
        // the commands that never touch it.
        std::fs::write(fx.primary.join("NEW"), "untracked\n").expect("untracked file");
        let swept = git_guard_decision(&env, &bash(&fx.primary, "git clean -fd")).expect("denied");
        assert!(swept.contains("no way back"), "{swept}");
        let reset =
            git_guard_decision(&env, &bash(&fx.primary, "git reset --hard")).expect("denied");
        assert!(reset.contains("Held by rescue/test:"), "{reset}");
    }

    /// `submodule.<name>.ignore` hides dirty submodule content from both the
    /// status and the diff, so the two agreed while content that no snapshot
    /// held was still lost.
    #[test]
    fn a_masked_dirty_submodule_is_never_reported_as_snapshotted() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        let sub = fx.root.join("sub");
        std::fs::create_dir(&sub).expect("mkdir sub");
        run_git(&sub, &["init", "--quiet"]);
        run_git(&sub, &["config", "user.email", "guard@example.com"]);
        run_git(&sub, &["config", "user.name", "guard"]);
        std::fs::write(sub.join("s.txt"), "s\n").expect("write s.txt");
        run_git(&sub, &["add", "s.txt"]);
        run_git(&sub, &["commit", "--quiet", "-m", "s"]);
        run_git(
            &fx.primary,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                sub.to_str().expect("utf8"),
                "sub",
            ],
        );
        run_git(&fx.primary, &["commit", "--quiet", "-m", "add sub"]);
        dirty_readme(&fx.primary);
        rescue_branch(&fx.primary, "rescue/test");
        // The mask, plus content that lives only in the submodule's tree.
        run_git(&fx.primary, &["config", "submodule.sub.ignore", "all"]);
        std::fs::write(fx.primary.join("sub/s.txt"), "irreplaceable\n").expect("dirty submodule");

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git reset --hard"))
            .expect("a dirty shared checkout is denied");
        assert!(reason.contains("no way back"), "{reason}");
        assert!(!reason.contains("rescue/test"), "{reason}");
    }

    /// `--sort=-committerdate` reads every ref's object, so one rescue ref with
    /// a missing object failed the whole call and took every good ref with it.
    #[test]
    fn a_broken_rescue_ref_does_not_hide_the_good_ones() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        dirty_readme(&fx.primary);
        rescue_branch(&fx.primary, "rescue/2026-07-27-0400");
        std::fs::write(
            fx.primary.join(".git/refs/heads/rescue/2026-07-27-0500"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        )
        .expect("plant a broken ref");

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git reset --hard"))
            .expect("a dirty shared checkout is denied");
        assert!(
            reason.contains("Held by rescue/2026-07-27-0400:"),
            "{reason}"
        );
    }

    /// An assume-unchanged path is invisible to `git status` and to `git diff`,
    /// but not to `git reset --hard`. Trusting the guard's own status list let
    /// the message promise a way back for content in no commit.
    #[test]
    fn an_assume_unchanged_edit_is_never_reported_as_snapshotted() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        std::fs::write(fx.primary.join("HIDDEN"), "original\n").expect("write HIDDEN");
        run_git(&fx.primary, &["add", "HIDDEN"]);
        run_git(&fx.primary, &["commit", "--quiet", "-m", "hidden"]);
        dirty_readme(&fx.primary);
        rescue_branch(&fx.primary, "rescue/test");
        run_git(
            &fx.primary,
            &["update-index", "--assume-unchanged", "HIDDEN"],
        );
        std::fs::write(fx.primary.join("HIDDEN"), "irreplaceable\n").expect("hidden edit");

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git reset --hard"))
            .expect("a dirty shared checkout is denied");
        assert!(reason.contains("no way back"), "{reason}");
        assert!(!reason.contains("rescue/test"), "{reason}");
    }

    /// `status.showUntrackedFiles = no` is a common large-repo setting and it
    /// hides the files the check has to see, so the modes are spelled out
    /// rather than inherited.
    #[test]
    fn untracked_files_hidden_by_config_still_block_the_snapshot_claim() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        run_git(&fx.primary, &["config", "status.showUntrackedFiles", "no"]);
        dirty_readme(&fx.primary);
        rescue_branch(&fx.primary, "rescue/test");
        std::fs::write(fx.primary.join("NEW"), "not in the snapshot\n").expect("untracked file");

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git clean -fd"))
            .expect("a dirty shared checkout is denied");
        assert!(reason.contains("no way back"), "{reason}");
        assert!(!reason.contains("rescue/test"), "{reason}");
    }

    /// A staged blob is in the object database whatever else is true, so the
    /// "None of it is staged" premise cannot be stated unconditionally either.
    #[test]
    fn the_staged_claim_matches_the_index() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        dirty_readme(&fx.primary);
        let unstaged =
            git_guard_decision(&env, &bash(&fx.primary, "git reset --hard")).expect("denied");
        assert!(unstaged.contains("None of it is staged"), "{unstaged}");

        run_git(&fx.primary, &["add", "README"]);
        let staged =
            git_guard_decision(&env, &bash(&fx.primary, "git reset --hard")).expect("denied");
        assert!(!staged.contains("None of it is staged"), "{staged}");
        assert!(
            staged.contains("Some of it is staged, so those blobs are in the object database"),
            "{staged}"
        );
    }

    /// The shared index is not the guard's to refresh: `git diff` rewrites its
    /// stat cache, so the check has to work off a copy.
    #[test]
    fn the_check_leaves_the_shared_index_untouched() {
        let Some(Protected { fx, env }) = snapshotted() else {
            return;
        };
        // Drop the stat cache so any refresh would rewrite the file.
        run_git(&fx.primary, &["read-tree", "HEAD"]);
        let index = fx.primary.join(".git/index");
        let before = std::fs::read(&index).expect("read index");

        let reason = git_guard_decision(&env, &bash(&fx.primary, "git reset --hard"))
            .expect("a dirty shared checkout is denied");
        assert!(reason.contains("Held by rescue/test:"), "{reason}");
        assert_eq!(
            before,
            std::fs::read(&index).expect("read index"),
            "the guard rewrote the shared index"
        );
    }

    #[test]
    fn read_only_git_in_a_protected_checkout_is_allowed() {
        let Some(Protected { fx, env }) = protected_fixture() else {
            return;
        };
        for cmd in [
            "git status --porcelain",
            "git log --oneline -5",
            "git diff",
            "git show HEAD",
            "git rev-parse HEAD",
            "git ls-files",
            "git branch",
            "git branch -a",
            "git worktree list",
            "git fetch origin",
            "git grep -n seed",
            "git add --dry-run .",
            "git clean -nd",
            "git stash list",
            "git stash create",
            "git apply --check /tmp/p.patch",
            "git worktree add /tmp/worktree/o/r/n -b b origin/main",
            // A literal mention is not a command.
            "echo 'git add -A'",
        ] {
            assert_eq!(
                git_guard_decision(&env, &bash(&fx.primary, cmd)),
                None,
                "{cmd}"
            );
        }
    }

    #[test]
    fn mutating_git_in_a_linked_worktree_is_allowed() {
        if !git_available() {
            return;
        }
        let fx = fixture();
        let linked = fx.root.join("wt");
        run_git(
            &fx.primary,
            &[
                "worktree",
                "add",
                "--quiet",
                linked.to_str().expect("utf8"),
                "-b",
                "topic",
            ],
        );
        // The glob deliberately covers the linked worktree as well, so the
        // only thing that can allow it is the private-git-dir check.
        let env = guard_env(&[&format!("{}/*", fx.root.display())]);
        assert!(
            git_guard_decision(&env, &bash(&fx.primary, "git add -A")).is_some(),
            "the primary is still protected under the same glob"
        );
        for cmd in ["git add -A", "git commit -am wip", "git reset --hard"] {
            assert_eq!(git_guard_decision(&env, &bash(&linked, cmd)), None, "{cmd}");
        }
    }

    #[test]
    fn kill_switch_and_empty_list_disable_the_guard() {
        let Some(Protected { fx, mut env }) = protected_fixture() else {
            return;
        };
        assert!(
            git_guard_decision(&env, &bash(&fx.primary, "git add -A")).is_some(),
            "armed"
        );
        // CLAUDE_CODE_DISABLE_GIT_GUARD.
        env.disabled = true;
        assert_eq!(
            git_guard_decision(&env, &bash(&fx.primary, "git add -A")),
            None
        );
        // No `primaryCheckouts` means there is nothing to protect.
        env.disabled = false;
        env.protected.clear();
        assert_eq!(
            git_guard_decision(&env, &bash(&fx.primary, "git add -A")),
            None
        );
    }

    #[test]
    fn malformed_input_fails_open() {
        // A glob that matches nothing the payloads below name: an unreadable
        // payload must not be rescued into an allow by luck.
        let env = guard_env(&["/no/such/protected/*"]);
        for payload in [
            json!({}),
            json!({"tool_name": "Bash"}),
            json!({"tool_name": "Bash", "tool_input": {}}),
            // Not the Bash tool at all.
            json!({"tool_name": "Edit", "cwd": "/", "tool_input": {"command": "git add -A"}}),
            // Wrongly typed fields.
            json!({"tool_name": 7, "cwd": "/", "tool_input": {"command": "git add -A"}}),
            json!({"tool_name": "Bash", "cwd": 7, "tool_input": {"command": "git add -A"}}),
            json!({"tool_name": "Bash", "cwd": "", "tool_input": {"command": "git add -A"}}),
            json!({"tool_name": "Bash", "cwd": "/", "tool_input": {"command": 7}}),
            json!({"tool_name": "Bash", "cwd": "/", "tool_input": "git add -A"}),
            json!([]),
            json!("git add -A"),
            Value::Null,
        ] {
            assert_eq!(git_guard_decision(&env, &payload), None, "{payload}");
        }
        // The JSON that never parses at all never reaches the decision: the
        // hook's `payload()` returns None and `git_guard` returns silently.
        assert!(serde_json::from_str::<Value>("{not json").is_err());
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
