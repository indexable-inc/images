//! Live SSH context for a coding agent: the user's configured host aliases
//! (from ~/.ssh/config and any Include files) and recently used ssh targets
//! (from the nushell history sqlite).
//!
//! Bundled with the `ssh-hosts` Claude skill and run on demand, so the output
//! reflects the current machine rather than a snapshot baked into the skill
//! text. History is read through the `sqlite3` CLI, which the Nix wrapper puts
//! on PATH.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct HostBlock {
    aliases: Vec<String>,
    hostname: String,
    user: String,
    port: String,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Resolve an `Include` token to concrete paths. Relative tokens resolve
/// against ~/.ssh; a `*`/`?` in the final component is matched against that
/// directory's entries.
fn resolve_include(token: &str) -> Vec<PathBuf> {
    let base: PathBuf = if let Some(rest) = token.strip_prefix("~/") {
        home().join(rest)
    } else if token.starts_with('/') {
        PathBuf::from(token)
    } else {
        home().join(".ssh").join(token)
    };

    let name = base.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name.contains('*') || name.contains('?') {
        let dir = base.parent().unwrap_or_else(|| Path::new("/"));
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Some(fname) = entry.file_name().to_str() {
                    if wildcard_match(name, fname) {
                        out.push(entry.path());
                    }
                }
            }
        }
        out.sort();
        out
    } else {
        vec![base]
    }
}

/// Minimal single-component glob: `*` matches any run, `?` matches one char.
/// No `**` or character classes; that covers ssh_config `Include` in practice.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    fn matches(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => matches(&p[1..], t) || (!t.is_empty() && matches(p, &t[1..])),
            (Some(b'?'), Some(_)) => matches(&p[1..], &t[1..]),
            (Some(pc), Some(tc)) if pc == tc => matches(&p[1..], &t[1..]),
            _ => false,
        }
    }
    matches(pattern.as_bytes(), text.as_bytes())
}

/// Parse one ssh_config file into host blocks, recursing into `Include`s where
/// they appear. Only Host/HostName/User/Port are tracked; that is enough to
/// pick and use an alias.
fn include_identity(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => path.to_path_buf(),
    }
}

fn parse_config(path: &Path, visited: &mut HashSet<PathBuf>, out: &mut Vec<HostBlock>) {
    if !visited.insert(include_identity(path)) {
        return;
    }

    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let mut cur: Option<HostBlock> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(keyword) = tokens.next() else {
            continue;
        };
        let rest: Vec<String> = tokens.map(str::to_string).collect();
        match keyword.to_ascii_lowercase().as_str() {
            "include" => {
                if let Some(block) = cur.take() {
                    out.push(block);
                }
                for token in &rest {
                    for included in resolve_include(token) {
                        parse_config(&included, visited, out);
                    }
                }
            }
            "host" => {
                if let Some(block) = cur.take() {
                    out.push(block);
                }
                cur = Some(HostBlock {
                    aliases: rest,
                    hostname: "-".into(),
                    user: "-".into(),
                    port: "-".into(),
                });
            }
            "hostname" => {
                if let (Some(block), Some(value)) = (cur.as_mut(), rest.first()) {
                    block.hostname = value.clone();
                }
            }
            "user" => {
                if let (Some(block), Some(value)) = (cur.as_mut(), rest.first()) {
                    block.user = value.clone();
                }
            }
            "port" => {
                if let (Some(block), Some(value)) = (cur.as_mut(), rest.first()) {
                    block.port = value.clone();
                }
            }
            _ => {}
        }
    }
    if let Some(block) = cur.take() {
        out.push(block);
    }
}

/// Recently used ssh command lines, newest first, deduplicated. Reads the
/// nushell history sqlite via the `sqlite3` CLI; returns empty on any failure
/// so the aliases section still prints.
fn recent_ssh() -> Vec<String> {
    let db = home().join("Library/Application Support/nushell/history.sqlite3");
    if !db.exists() {
        return Vec::new();
    }
    let sql = "SELECT command_line FROM history WHERE command_line GLOB 'ssh *' \
               GROUP BY command_line ORDER BY max(start_timestamp) DESC LIMIT 15;";
    let Ok(output) = Command::new("sqlite3").arg(&db).arg(sql).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn main() {
    let mut blocks = Vec::new();
    let mut visited = HashSet::new();
    parse_config(&home().join(".ssh/config"), &mut visited, &mut blocks);

    println!("## SSH host aliases — ~/.ssh/config (+ Includes)\n");
    println!("| aliases | hostname | user | port |");
    println!("| --- | --- | --- | --- |");
    let mut printed = 0usize;
    for block in &blocks {
        let aliases: Vec<&str> = block
            .aliases
            .iter()
            .map(String::as_str)
            .filter(|alias| !alias.contains('*') && !alias.contains('?'))
            .collect();
        if aliases.is_empty() {
            continue;
        }
        printed += 1;
        println!(
            "| {} | {} | {} | {} |",
            aliases.join(", "),
            block.hostname,
            block.user,
            block.port
        );
    }
    if printed == 0 {
        println!("| _(none found)_ | | | |");
    }

    println!("\n## Recent ssh commands (newest first)\n");
    let recent = recent_ssh();
    if recent.is_empty() {
        println!("_(no ssh history found)_");
    } else {
        for command in recent {
            println!("- {command}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> Result<PathBuf, Box<dyn Error>> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ssh-hosts-{name}-{nonce}"));
        fs::create_dir(&path)?;
        Ok(path)
    }

    fn config_with_include(alias: &str, include: &Path) -> String {
        format!(
            "Host {alias}\n  HostName {alias}.example\nInclude {}\n",
            include.display()
        )
    }

    #[test]
    fn parse_config_skips_cyclic_includes() -> Result<(), Box<dyn Error>> {
        let dir = temp_dir("cycle")?;
        let root = dir.join("config");
        let child = dir.join("config-extra");
        fs::write(&root, config_with_include("root", &child))?;
        fs::write(&child, config_with_include("child", &root))?;

        let mut blocks = Vec::new();
        let mut visited = HashSet::new();
        parse_config(&root, &mut visited, &mut blocks);

        let aliases: Vec<&str> = blocks
            .iter()
            .flat_map(|block| block.aliases.iter().map(String::as_str))
            .collect();
        assert_eq!(aliases, ["root", "child"]);

        fs::remove_dir_all(dir)?;
        Ok(())
    }
}
