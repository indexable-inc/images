//! `sqlmerge <base> <ours> <theirs>`: a git merge driver for `SQLite` files.
//!
//! git invokes this as the `%O %A %B` triple: `%O` is the common ancestor
//! (base), `%A` is our version (rewritten in place with the merge result), and
//! `%B` is their version. Exit 0 means a clean merge was written to `<ours>`;
//! exit 1 means conflict or refusal, and git then marks the file conflicted.

use std::process::ExitCode;

use sqlmerge::{ConflictPolicy, merge};

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

    match merge(base, ours, theirs, ConflictPolicy::Abort) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sqlmerge: {e}");
            ExitCode::FAILURE
        }
    }
}
