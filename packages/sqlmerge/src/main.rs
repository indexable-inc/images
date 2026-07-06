//! `sqlmerge <base> <ours> <theirs>`: a git merge driver for `SQLite` files.
//!
//! git invokes this as the `%O %A %B` triple: `%O` is the common ancestor
//! (base), `%A` is our version (rewritten in place with the merge result), and
//! `%B` is their version. Exit 0 means a clean merge was written to `<ours>`;
//! exit 1 means conflict or refusal, and git then marks the file conflicted.

use std::process::ExitCode;

use sqlmerge::{PolicyConfig, merge};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, base, ours, theirs] = args.as_slice() else {
        eprintln!(
            "usage: sqlmerge <base> <ours> <theirs>\n\
             \n\
             git merge driver for SQLite databases. Wire it up with:\n\
             \n\
             .gitattributes:    *.db merge=sqlite\n\
             git config:        [merge \"sqlite\"]\n\
             \x20                    name = SQLite three-way merge\n\
             \x20                    driver = sqlmerge %O %A %B"
        );
        return ExitCode::FAILURE;
    };

    // Resolve per-table policy from `sqlmerge.toml` at the repo root. git runs
    // the driver with CWD = the worktree root, so we walk up from CWD to find
    // it. Absent file = abort every conflict (the pre-config default). A
    // malformed config is a loud refusal, never a silent fall-back to abort.
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("sqlmerge: cannot determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let policies = match PolicyConfig::load_from(&cwd) {
        Ok(policies) => policies,
        Err(e) => {
            eprintln!("sqlmerge: {e}");
            return ExitCode::FAILURE;
        }
    };

    match merge(base, ours, theirs, &policies) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sqlmerge: {e}");
            ExitCode::FAILURE
        }
    }
}
