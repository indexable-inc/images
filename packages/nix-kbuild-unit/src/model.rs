//! Plan JSON model shared by `harvest` (writer) and `render` (reader).

use std::collections::BTreeMap;

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
    /// Module base objects from `modules.order` (e.g. `drivers/net/dummy.o`),
    /// in file order. modpost's member list lives in this file rather than in
    /// its argv, so dep wiring for `Module.symvers` needs it in the plan.
    /// Empty (and absent from the JSON) when `CONFIG_MODULES` is off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
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
    /// Contents of `@file` response files the command references, keyed by
    /// objtree-relative path. `ld` expands a response file as extra argv
    /// (`cmd_ld_multi_m` links a multi-object module from `@<mod>.mod`), so
    /// dep wiring must scan these tokens like command tokens; the file itself
    /// rides in the generated snapshot for the replay to read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub at_file_contents: BTreeMap<String, String>,
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
    /// `ld -r` object aggregation: `vmlinux.o`, and each multi-object
    /// module's `<mod>.o` linked from its members.
    ObjectAggregate,
    /// `scripts/link-vmlinux.sh` plus the arch postlink make.
    Link,
    /// `scripts/mod/modpost` export check over `vmlinux.o`
    /// (`vmlinux.symvers`, plus `.vmlinux.export.c` on module builds).
    Modpost,
    /// The `make modules` modpost pass (`Module.symvers`): reads `vmlinux.o`
    /// and every module object listed in `modules.order`, writes the symvers
    /// dump and each module's `<mod>.mod.c`.
    ModpostModules,
    /// Final `ld -r -T scripts/module.lds` of one `.ko` from its module
    /// object, `<mod>.mod.o`, and `.module-common.o`.
    ModuleLink,
    /// A savedcmd-only object rule with no dep tracking (`if_changed`, not
    /// `if_changed_dep`): EFI libstub's stubcopy strip/objcopy at defconfig
    /// scale. Replays the exact argv over the unit outputs it references.
    Command,
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
            "vmlinux.symvers" => Some(UnitKind::Modpost),
            "Module.symvers" => Some(UnitKind::ModpostModules),
            _ if target.ends_with(".ko") => Some(UnitKind::ModuleLink),
            _ if target.ends_with(".o")
                && !target.starts_with("scripts/")
                && !target.starts_with("tools/") =>
            {
                if self.source.is_some() {
                    Some(UnitKind::Compile)
                } else if is_ld_r_aggregation(&self.cmd) {
                    Some(UnitKind::ObjectAggregate)
                } else {
                    Some(UnitKind::Command)
                }
            }
            _ if target.ends_with(".a") => Some(UnitKind::Archive),
            _ => None,
        }
    }
}

/// Whether a source-less object rule is an `ld -r` aggregation (`vmlinux.o`,
/// multi-object module `.o`) rather than an opaque copy command.
fn is_ld_r_aggregation(cmd: &str) -> bool {
    let mut tokens = cmd.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    let linker = first.rsplit('/').next().unwrap_or(first);
    (linker == "ld" || linker.starts_with("ld.")) && tokens.any(|token| token == "-r")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(target: &str, cmd: &str, source: Option<&str>) -> CmdEntry {
        CmdEntry {
            target: target.to_owned(),
            cmd: cmd.to_owned(),
            source: source.map(str::to_owned),
            deps: Vec::new(),
            config_deps: Vec::new(),
            at_file_contents: BTreeMap::new(),
        }
    }

    #[test]
    fn classifies_targets() {
        assert_eq!(
            entry(
                "kernel/fork.o",
                "gcc -c -o kernel/fork.o kernel/fork.c",
                Some("kernel/fork.c")
            )
            .unit_kind(),
            Some(UnitKind::Compile)
        );
        assert_eq!(
            entry("lib/lib.a", "rm -f lib/lib.a; ar cDPrST lib/lib.a", None).unit_kind(),
            Some(UnitKind::Archive)
        );
        assert_eq!(
            entry(
                "vmlinux.o",
                "ld -m elf_i386 -r -o vmlinux.o --whole-archive vmlinux.a",
                None
            )
            .unit_kind(),
            Some(UnitKind::ObjectAggregate)
        );
        assert_eq!(
            entry(
                "vmlinux",
                "scripts/link-vmlinux.sh \"ld\"",
                Some("scripts/link-vmlinux.sh")
            )
            .unit_kind(),
            Some(UnitKind::Link)
        );
        assert_eq!(
            entry(
                "vmlinux.symvers",
                "scripts/mod/modpost -M -E -o vmlinux.symvers vmlinux.o",
                None
            )
            .unit_kind(),
            Some(UnitKind::Modpost)
        );
        // Host-tool objects and generated sources are plan-build-only.
        assert_eq!(
            entry(
                "scripts/mod/empty.o",
                "gcc -c -o scripts/mod/empty.o scripts/mod/empty.c",
                Some("scripts/mod/empty.c")
            )
            .unit_kind(),
            None
        );
        assert_eq!(
            entry(
                "arch/x86/kernel/vmlinux.lds",
                "gcc -E -o arch/x86/kernel/vmlinux.lds",
                Some("arch/x86/kernel/vmlinux.lds.S")
            )
            .unit_kind(),
            None
        );
        // An .o with a savedcmd but no dep tracking (`if_changed` rules)
        // replays as an opaque command unit; ones nothing references (the
        // link script rebuilds init/version-timestamp.o itself) get pruned.
        assert_eq!(
            entry(
                "drivers/firmware/efi/libstub/alignedmem.stub.o",
                "strip --strip-debug -o drivers/firmware/efi/libstub/alignedmem.stub.o \
                 drivers/firmware/efi/libstub/alignedmem.o",
                None
            )
            .unit_kind(),
            Some(UnitKind::Command)
        );
        assert_eq!(
            entry(
                "init/version-timestamp.o",
                "gcc -c -o init/version-timestamp.o init/version-timestamp.c",
                None
            )
            .unit_kind(),
            Some(UnitKind::Command)
        );
    }

    #[test]
    fn classifies_module_targets() {
        assert_eq!(
            entry(
                "Module.symvers",
                "scripts/mod/modpost -M -E -o Module.symvers -T modules.order vmlinux.o",
                None
            )
            .unit_kind(),
            Some(UnitKind::ModpostModules)
        );
        assert_eq!(
            entry(
                "drivers/net/dummy.ko",
                "ld -r -T scripts/module.lds -o drivers/net/dummy.ko drivers/net/dummy.o \
                 drivers/net/dummy.mod.o .module-common.o",
                None
            )
            .unit_kind(),
            Some(UnitKind::ModuleLink)
        );
        // A multi-object module's <mod>.o is an `ld -r` aggregation over the
        // response-file member list, exactly like vmlinux.o over archives.
        assert_eq!(
            entry(
                "fs/nfs/nfs.o",
                "ld -m elf_x86_64 -z noexecstack -r -o fs/nfs/nfs.o @fs/nfs/nfs.mod",
                None
            )
            .unit_kind(),
            Some(UnitKind::ObjectAggregate)
        );
        // 6.12 links scripts/module-common.c per-ko as a root-level dotfile
        // object with dep tracking: a plain compile unit.
        assert_eq!(
            entry(
                ".module-common.o",
                "gcc -c -o .module-common.o scripts/module-common.c",
                Some("scripts/module-common.c")
            )
            .unit_kind(),
            Some(UnitKind::Compile)
        );
        assert_eq!(
            entry(
                ".vmlinux.export.o",
                "gcc -c -o .vmlinux.export.o .vmlinux.export.c",
                Some(".vmlinux.export.c")
            )
            .unit_kind(),
            Some(UnitKind::Compile)
        );
    }

    #[test]
    fn classifies_captured_defconfig_module_cmds() {
        let classify = |text: &str| {
            let parsed = crate::cmd_file::parse(text).expect("parse fixture");
            CmdEntry {
                target: parsed.target,
                cmd: parsed.cmd,
                source: parsed.source,
                deps: parsed.deps,
                config_deps: parsed.config_deps,
                at_file_contents: BTreeMap::new(),
            }
            .unit_kind()
        };
        for (fixture, expected) in [
            (
                include_str!("../testdata/defconfig-6.12/efivarfs.o.cmd"),
                UnitKind::ObjectAggregate,
            ),
            (
                include_str!("../testdata/defconfig-6.12/efivarfs.mod.o.cmd"),
                UnitKind::Compile,
            ),
            (
                include_str!("../testdata/defconfig-6.12/efivarfs.ko.cmd"),
                UnitKind::ModuleLink,
            ),
            (
                include_str!("../testdata/defconfig-6.12/Module.symvers.cmd"),
                UnitKind::ModpostModules,
            ),
            (
                include_str!("../testdata/defconfig-6.12/vmlinux.symvers.cmd"),
                UnitKind::Modpost,
            ),
            (
                include_str!("../testdata/defconfig-6.12/module-common.o.cmd"),
                UnitKind::Compile,
            ),
            (
                include_str!("../testdata/defconfig-6.12/vmlinux.export.o.cmd"),
                UnitKind::Compile,
            ),
        ] {
            assert_eq!(classify(fixture), Some(expected));
        }
    }

    #[test]
    fn plan_json_omits_module_fields_when_module_less() {
        // The module-less plan must serialize byte-identically to P2's: the
        // tinyconfig plan output is content-addressed, and equality is what
        // lets every existing unit realisation cut off.
        let plan = Plan {
            kernel_release: "6.12.95".to_owned(),
            cmds: vec![entry("built-in.a", "ar cDPrST built-in.a", None)],
            generated: Vec::new(),
            modules: Vec::new(),
        };
        let json = serde_json::to_string(&plan).expect("serialize plan");
        assert!(!json.contains("modules"));
        assert!(!json.contains("at_file_contents"));
    }
}
