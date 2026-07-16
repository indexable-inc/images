//! Plan JSON model shared by `harvest` (writer) and `render` (reader).

use serde::{Deserialize, Serialize};

/// Everything stage 2 needs to render per-unit derivations: the parsed saved
/// commands plus the manifest of build-created files the plan derivation
/// snapshots for unit build trees.
#[derive(Debug, Serialize, Deserialize)]
pub struct Plan {
    /// `include/config/kernel.release` from the plan build.
    pub kernel_release: String,
    /// Parsed `.cmd` files, sorted by target.
    pub cmds: Vec<CmdEntry>,
    /// Objtree-relative paths snapshotted into the generated-file tree,
    /// sorted.
    pub generated: Vec<String>,
}

/// One parsed `.cmd` file (see `cmd_file`).
#[derive(Debug, Serialize, Deserialize)]
pub struct CmdEntry {
    /// Objtree-relative build target.
    pub target: String,
    /// The exact shell command kbuild ran, make-unescaped.
    pub cmd: String,
    /// Primary source file, when dep tracking recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Tracked prerequisite files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    /// `include/config/*` CONFIG dependency markers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_deps: Vec<String>,
}

/// How a `.cmd` target replays as a derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    /// One translation unit: a compiler invocation producing an object file.
    /// When objtool is configured in, it rides inside the saved command, so
    /// the replay covers it with no extra handling.
    Compile,
    /// `ar` thin-archive aggregation (`built-in.a`, `lib.a`, `vmlinux.a`).
    Archive,
    /// `ld -r` object aggregation (`vmlinux.o`).
    ObjectAggregate,
    /// `scripts/link-vmlinux.sh` plus the arch postlink make.
    Link,
    /// `scripts/mod/modpost` export check over `vmlinux.o`.
    Modpost,
}

impl CmdEntry {
    /// Classify how this saved command replays, or `None` when the target is
    /// plan-build-only (host tools, generated sources, vdso/realmode
    /// intermediates) and its product ships in the generated snapshot instead.
    #[must_use]
    pub fn unit_kind(&self) -> Option<UnitKind> {
        let target = self.target.as_str();
        match target {
            "vmlinux" => Some(UnitKind::Link),
            "vmlinux.o" => Some(UnitKind::ObjectAggregate),
            "vmlinux.symvers" => Some(UnitKind::Modpost),
            _ if target.ends_with(".a") => Some(UnitKind::Archive),
            _ if target.ends_with(".o")
                && self.source.is_some()
                && !target.starts_with("scripts/")
                && !target.starts_with("tools/") =>
            {
                Some(UnitKind::Compile)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(target: &str, source: Option<&str>) -> CmdEntry {
        CmdEntry {
            target: target.to_owned(),
            cmd: String::new(),
            source: source.map(str::to_owned),
            deps: Vec::new(),
            config_deps: Vec::new(),
        }
    }

    #[test]
    fn classifies_targets() {
        assert_eq!(
            entry("kernel/fork.o", Some("kernel/fork.c")).unit_kind(),
            Some(UnitKind::Compile)
        );
        assert_eq!(
            entry("lib/lib.a", None).unit_kind(),
            Some(UnitKind::Archive)
        );
        assert_eq!(
            entry("vmlinux.o", None).unit_kind(),
            Some(UnitKind::ObjectAggregate)
        );
        assert_eq!(
            entry("vmlinux", Some("scripts/link-vmlinux.sh")).unit_kind(),
            Some(UnitKind::Link)
        );
        assert_eq!(
            entry("vmlinux.symvers", None).unit_kind(),
            Some(UnitKind::Modpost)
        );
        // Host-tool objects and generated sources are plan-build-only.
        assert_eq!(
            entry("scripts/mod/empty.o", Some("scripts/mod/empty.c")).unit_kind(),
            None
        );
        assert_eq!(
            entry(
                "arch/x86/kernel/vmlinux.lds",
                Some("arch/x86/kernel/vmlinux.lds.S")
            )
            .unit_kind(),
            None
        );
        // An .o with no dep tracking (compiled inside a script) is not a unit.
        assert_eq!(entry("init/version-timestamp.o", None).unit_kind(), None);
    }
}
