//! Parser for kbuild `.cmd` files, the `.<name>.cmd` sidecar fixdep writes
//! next to each build product.
//!
//! Format:
//!
//! ```text
//! savedcmd_<target> := <shell command, make-escaped>
//! source_<target> := <path>
//! deps_<target> := \
//!     $(wildcard include/config/FOO) \
//!   include/linux/foo.h \
//! <target>: $(deps_<target>)
//! ```
//!
//! `source_`/`deps_` appear only on targets with dep tracking (fixdep ran).
//! `make-cmd` (scripts/Kbuild.include) escapes the saved command for make:
//! `$` becomes `$$` and `#` becomes `$(pound)`.

use color_eyre::eyre::{OptionExt as _, bail};

/// One parsed `.cmd` file.
#[derive(Debug, PartialEq, Eq)]
pub struct CmdFile {
    /// Objtree-relative build target, e.g. `kernel/fork.o`.
    pub target: String,
    /// The exact shell command kbuild ran, make-unescaped.
    pub cmd: String,
    /// Primary source file (`source_<target>`), when dep tracking recorded one.
    pub source: Option<String>,
    /// Tracked prerequisite files (headers, sources, linker scripts).
    pub deps: Vec<String>,
    /// `include/config/*` markers fixdep emits for CONFIG dependencies.
    pub config_deps: Vec<String>,
}

const SAVEDCMD: &str = "savedcmd_";

pub fn parse(text: &str) -> color_eyre::Result<CmdFile> {
    let mut lines = text.lines();
    let first = lines.next().ok_or_eyre("empty .cmd file")?;
    let Some(rest) = first.strip_prefix(SAVEDCMD) else {
        bail!("expected `{SAVEDCMD}<target> := ...` on the first line, got {first:?}");
    };
    let (target, escaped_cmd) = rest
        .split_once(" := ")
        .ok_or_eyre("missing ` := ` after savedcmd target")?;
    let target = target.to_owned();
    let cmd = unescape_make(escaped_cmd.trim());

    let mut source = None;
    let mut deps = Vec::new();
    let mut config_deps = Vec::new();

    let source_prefix = format!("source_{target} := ");
    let deps_header = format!("deps_{target} := \\");

    while let Some(line) = lines.next() {
        if let Some(path) = line.strip_prefix(&source_prefix) {
            if source.replace(path.to_owned()).is_some() {
                bail!("duplicate source_{target} line");
            }
        } else if line == deps_header {
            for dep_line in lines.by_ref() {
                let entry = dep_line.trim().trim_end_matches('\\').trim_end();
                if entry.is_empty() {
                    break;
                }
                if let Some(config) = entry
                    .strip_prefix("$(wildcard ")
                    .and_then(|inner| inner.strip_suffix(')'))
                {
                    config_deps.push(config.to_owned());
                } else if entry.contains("$(") {
                    bail!("unrecognized make construct in deps_{target}: {entry:?}");
                } else {
                    deps.push(entry.to_owned());
                }
            }
        }
    }

    Ok(CmdFile {
        target,
        cmd,
        source,
        deps,
        config_deps,
    })
}

/// Undo `make-cmd` escaping: `$$` is a literal `$`, `$(pound)` a literal `#`.
fn unescape_make(escaped: &str) -> String {
    let mut out = String::with_capacity(escaped.len());
    let mut rest = escaped;
    while let Some(idx) = rest.find('$') {
        out.push_str(&rest[..idx]);
        let tail = &rest[idx..];
        if let Some(after) = tail.strip_prefix("$$") {
            out.push('$');
            rest = after;
        } else if let Some(after) = tail.strip_prefix("$(pound)") {
            out.push('#');
            rest = after;
        } else {
            // A lone `$` never appears in a make-escaped command; preserve it
            // verbatim rather than guessing.
            out.push('$');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORK: &str = include_str!("../testdata/tinyconfig-6.12/kernel-fork.o.cmd");
    const ENTRY_32: &str = include_str!("../testdata/tinyconfig-6.12/entry_32.o.cmd");
    const TOP_BUILT_IN: &str = include_str!("../testdata/tinyconfig-6.12/top-built-in.a.cmd");
    const EMPTY_BUILT_IN: &str = include_str!("../testdata/tinyconfig-6.12/empty-built-in.a.cmd");
    const VMLINUX_A: &str = include_str!("../testdata/tinyconfig-6.12/vmlinux.a.cmd");
    const VMLINUX: &str = include_str!("../testdata/tinyconfig-6.12/vmlinux.cmd");
    const VMLINUX_LDS: &str = include_str!("../testdata/tinyconfig-6.12/vmlinux.lds.cmd");

    #[test]
    fn parses_c_compile() {
        let parsed = parse(FORK).expect("parse kernel/fork.o cmd");
        assert_eq!(parsed.target, "kernel/fork.o");
        assert_eq!(parsed.source.as_deref(), Some("kernel/fork.c"));
        assert!(parsed.cmd.starts_with("gcc "));
        assert!(parsed.cmd.ends_with("-c -o kernel/fork.o kernel/fork.c"));
        assert!(parsed.deps.contains(&"include/linux/sched.h".to_owned()));
        assert!(parsed.deps.len() > 500, "got {} deps", parsed.deps.len());
        assert!(
            parsed
                .config_deps
                .contains(&"include/config/VMAP_STACK".to_owned())
        );
    }

    #[test]
    fn parses_asm_compile() {
        let parsed = parse(ENTRY_32).expect("parse entry_32.o cmd");
        assert_eq!(parsed.target, "arch/x86/entry/entry_32.o");
        assert_eq!(parsed.source.as_deref(), Some("arch/x86/entry/entry_32.S"));
        assert!(parsed.cmd.contains("-D__ASSEMBLY__"));
    }

    #[test]
    fn parses_archives() {
        let top = parse(TOP_BUILT_IN).expect("parse built-in.a cmd");
        assert_eq!(top.target, "built-in.a");
        assert!(top.cmd.contains("xargs ar cDPrST built-in.a"));
        assert_eq!(top.source, None);
        assert!(top.deps.is_empty());

        let empty = parse(EMPTY_BUILT_IN).expect("parse empty archive cmd");
        assert_eq!(empty.target, "drivers/nfc/built-in.a");
        assert!(empty.cmd.ends_with("ar cDPrST drivers/nfc/built-in.a"));
    }

    #[test]
    fn unescapes_dollar_expansions() {
        let parsed = parse(VMLINUX_A).expect("parse vmlinux.a cmd");
        assert_eq!(parsed.target, "vmlinux.a");
        assert!(parsed.cmd.contains("$(ar t vmlinux.a | sed -n 1p)"));
        assert!(!parsed.cmd.contains("$$"));
    }

    #[test]
    fn parses_link_with_config_only_deps() {
        let parsed = parse(VMLINUX).expect("parse vmlinux cmd");
        assert_eq!(parsed.target, "vmlinux");
        assert_eq!(parsed.source.as_deref(), Some("scripts/link-vmlinux.sh"));
        assert!(parsed.cmd.starts_with("scripts/link-vmlinux.sh \"ld\""));
        assert!(
            parsed
                .cmd
                .ends_with("make -f ./arch/x86/Makefile.postlink vmlinux")
        );
        assert!(parsed.deps.is_empty());
        assert!(
            parsed
                .config_deps
                .contains(&"include/config/KALLSYMS".to_owned())
        );
    }

    #[test]
    fn parses_generated_lds() {
        let parsed = parse(VMLINUX_LDS).expect("parse vmlinux.lds cmd");
        assert_eq!(parsed.target, "arch/x86/kernel/vmlinux.lds");
        assert_eq!(
            parsed.source.as_deref(),
            Some("arch/x86/kernel/vmlinux.lds.S")
        );
    }

    #[test]
    fn unescape_handles_pound_and_dollar() {
        assert_eq!(unescape_make("a $$b $(pound)c"), "a $b #c");
        assert_eq!(unescape_make("no escapes"), "no escapes");
    }

    #[test]
    fn rejects_non_cmd_input() {
        assert!(parse("not a cmd file\n").is_err());
    }
}
