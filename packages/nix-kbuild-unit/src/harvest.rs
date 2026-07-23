//! Stage 1: walk a completed kbuild objtree, parse every `.cmd` file, and
//! snapshot build-created non-unit files (the "generated" tree) that unit
//! replays overlay on top of the pristine source.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use color_eyre::eyre::{WrapErr as _, bail};

use crate::cmd_file;
use crate::model::{CmdEntry, Plan};

pub fn harvest(
    objtree: &Path,
    srctree: &Path,
    generated_out: Option<&Path>,
) -> color_eyre::Result<Plan> {
    let files = walk(objtree)?;

    let mut cmds = Vec::new();
    for rel in &files {
        if !is_cmd_file(rel) {
            continue;
        }
        let path = objtree.join(rel);
        let text = std::fs::read_to_string(&path)
            .wrap_err_with(|| format!("reading {}", path.display()))?;
        let parsed = cmd_file::parse(&text).wrap_err_with(|| format!("parsing {rel}"))?;
        let mut entry = CmdEntry {
            target: parsed.target,
            cmd: parsed.cmd,
            source: parsed.source,
            deps: parsed.deps,
            config_deps: parsed.config_deps,
            at_file_contents: BTreeMap::new(),
        };
        if entry.unit_kind().is_some() {
            entry.at_file_contents = at_file_contents(objtree, &entry.cmd)?;
        }
        cmds.push(entry);
    }
    cmds.sort_by(|a, b| a.target.cmp(&b.target));
    for pair in cmds.windows(2) {
        if pair[0].target == pair[1].target {
            bail!("duplicate .cmd target {}", pair[0].target);
        }
    }

    let unit_targets: BTreeSet<&str> = cmds
        .iter()
        .filter(|entry| entry.unit_kind().is_some())
        .map(|entry| entry.target.as_str())
        .collect();

    let mut generated = Vec::new();
    for rel in &files {
        if !in_generated_snapshot(rel, &unit_targets) {
            continue;
        }
        let obj_path = objtree.join(rel);
        let src_path = srctree.join(rel);
        // Build-created (absent in src) or modified in place both go into the
        // snapshot; the unit template overlays it with --remove-destination so
        // a modified file shadows its source symlink.
        let in_snapshot = match std::fs::symlink_metadata(&src_path) {
            Ok(_) => !same_entry(&obj_path, &src_path)?,
            Err(_) => true,
        };
        if in_snapshot {
            generated.push(rel.clone());
        }
    }

    if let Some(out) = generated_out {
        for rel in &generated {
            let dst = out.join(rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)
                    .wrap_err_with(|| format!("creating {}", parent.display()))?;
            }
            let src = objtree.join(rel);
            let meta = std::fs::symlink_metadata(&src)
                .wrap_err_with(|| format!("stat {}", src.display()))?;
            if meta.is_symlink() {
                // Recreate symlinks verbatim (kbuild makes dir-targeted links
                // like scripts/dtc/include-prefixes/*); copying would follow
                // them and fail on directories.
                let target = std::fs::read_link(&src)
                    .wrap_err_with(|| format!("readlink {}", src.display()))?;
                std::os::unix::fs::symlink(&target, &dst)
                    .wrap_err_with(|| format!("snapshotting symlink {rel}"))?;
            } else {
                // fs::copy preserves permission bits, so snapshotted host
                // tools stay executable.
                std::fs::copy(&src, &dst).wrap_err_with(|| format!("snapshotting {rel}"))?;
            }
        }
    }

    let release_path = objtree.join("include/config/kernel.release");
    let kernel_release = std::fs::read_to_string(&release_path)
        .wrap_err_with(|| {
            format!(
                "reading {} (is this a completed kbuild objtree?)",
                release_path.display()
            )
        })?
        .trim()
        .to_owned();

    let modules = read_modules_order(objtree)?;

    Ok(Plan {
        kernel_release,
        cmds,
        generated,
        modules,
    })
}

/// Read `modules.order` (module base objects, one per line) when the build
/// produced one; `CONFIG_MODULES=n` builds have none. The modpost member
/// list lives in this file rather than in the saved command's argv, so it
/// must ride in the plan for `Module.symvers` dep wiring.
fn read_modules_order(objtree: &Path) -> color_eyre::Result<Vec<String>> {
    let path = objtree.join("modules.order");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
    let entries: Vec<String> = text.lines().map(str::to_owned).collect();
    for entry in &entries {
        // 6.12 lists `<mod>.o` paths; bail loudly on anything else rather
        // than guessing at an unknown kernel's format.
        if !entry.ends_with(".o") {
            bail!("unrecognized modules.order entry {entry:?} (expected an .o path)");
        }
    }
    Ok(entries)
}

/// Capture the contents of `@file` response files a saved command references
/// (`cmd_ld_multi_m` links a multi-object module from `@<mod>.mod`). Only
/// tokens naming an existing objtree file expand; anything else (symbol
/// decorations, literal `@`) passes through untouched.
fn at_file_contents(objtree: &Path, cmd: &str) -> color_eyre::Result<BTreeMap<String, String>> {
    let mut contents = BTreeMap::new();
    for token in cmd.split_whitespace() {
        let Some(rel) = token.trim_matches('"').strip_prefix('@') else {
            continue;
        };
        let path = objtree.join(rel);
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .wrap_err_with(|| format!("reading response file {rel}"))?;
        contents.insert(rel.to_owned(), text);
    }
    Ok(contents)
}

/// Collect every regular file (and file symlink) under `root` as sorted
/// `/`-separated relative paths.
fn walk(root: &Path) -> color_eyre::Result<Vec<String>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).wrap_err_with(|| format!("listing {}", dir.display()))?;
        for entry in entries {
            let entry = entry.wrap_err_with(|| format!("listing {}", dir.display()))?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .expect("walked path is under its root");
            let Some(rel) = rel.to_str() else {
                bail!("non-UTF-8 path in objtree: {}", rel.display());
            };
            files.push(rel.to_owned());
        }
    }
    files.sort();
    Ok(files)
}

/// `.cmd` sidecars are dotfiles (`.fork.o.cmd`); `auto.conf.cmd` and friends
/// are make includes in a different format, not saved commands. Host tools
/// under `tools/` (objtool at defconfig scale) build with tools/build, whose
/// sidecar dialect is `cmd_<absolute target> :=` with no `savedcmd_` line;
/// nothing under `tools/` is ever a unit, and the built tools themselves
/// flow into the generated snapshot, so skip their sidecars entirely.
fn is_cmd_file(rel: &str) -> bool {
    if rel.starts_with("tools/") {
        return false;
    }
    let name = file_name(rel);
    name.starts_with('.') && name.ends_with(".cmd")
}

fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// Whether a build-created file belongs in the generated snapshot units build
/// against. Everything a unit produces or regenerates must stay out: writing
/// through a store symlink fails the unit build (loudly, by design).
fn in_generated_snapshot(rel: &str, unit_targets: &BTreeSet<&str>) -> bool {
    if unit_targets.contains(rel) {
        return false;
    }
    let name = file_name(rel);
    // Make bookkeeping, regenerated per build and never read by a unit.
    if name.ends_with(".cmd") || name.ends_with(".d") || name.starts_with(".tmp_") {
        return false;
    }
    // Modpost products without a .cmd of their own, regenerated by the
    // modpost units at replay time (so the unit_targets check above cannot
    // catch them): a store symlink here would break their ownership. The
    // `.mod` member lists and `modules.order` stay in: modpost and the
    // multi-object links read them, and the plan rerun regenerates them
    // whenever the module set changes.
    if name.ends_with(".mod.c") || rel == ".vmlinux.export.c" {
        return false;
    }
    // Objects and archives are unit products; object-shaped files that are
    // NOT units (host-tool objects, vdso pieces, init/version-timestamp.o
    // recompiled inside link-vmlinux.sh) are either dead at unit-build time
    // or rebuilt fresh in the unit tree.
    if rel.ends_with(".o") || rel.ends_with(".a") {
        return false;
    }
    // Link outputs, regenerated by the link unit. The arch postlink also
    // rewrites its relocation dump (`arch/x86/boot/compressed/vmlinux.relocs`
    // on CONFIG_RELOCATABLE builds), so the link unit owns that too. Only the
    // vmlinux dump: `realmode.relocs` is a plan-time product that rmpiggy.S
    // incbins from the snapshot.
    if rel == "System.map" || rel == "vmlinux.map" || name == "vmlinux.relocs" {
        return false;
    }
    true
}

fn same_entry(a: &Path, b: &Path) -> color_eyre::Result<bool> {
    let meta_a = std::fs::symlink_metadata(a).wrap_err_with(|| format!("stat {}", a.display()))?;
    let meta_b = std::fs::symlink_metadata(b).wrap_err_with(|| format!("stat {}", b.display()))?;
    if meta_a.is_symlink() || meta_b.is_symlink() {
        if !(meta_a.is_symlink() && meta_b.is_symlink()) {
            return Ok(false);
        }
        let link_a = std::fs::read_link(a).wrap_err_with(|| format!("readlink {}", a.display()))?;
        let link_b = std::fs::read_link(b).wrap_err_with(|| format!("readlink {}", b.display()))?;
        return Ok(link_a == link_b);
    }
    if meta_a.len() != meta_b.len() {
        return Ok(false);
    }
    let bytes_a = std::fs::read(a).wrap_err_with(|| format!("reading {}", a.display()))?;
    let bytes_b = std::fs::read(b).wrap_err_with(|| format!("reading {}", b.display()))?;
    Ok(bytes_a == bytes_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORK: &str = include_str!("../testdata/tinyconfig-6.12/kernel-fork.o.cmd");
    const TOP_BUILT_IN: &str = include_str!("../testdata/tinyconfig-6.12/top-built-in.a.cmd");

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("rel path has a parent"))
            .expect("create parent dir");
        std::fs::write(path, contents).expect("write fixture file");
    }

    #[test]
    fn harvests_cmds_and_generated_snapshot() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let srctree = tmp.path().join("src");
        let objtree = tmp.path().join("obj");
        let out = tmp.path().join("generated");

        write(&srctree, "kernel/fork.c", "int x;\n");
        write(&srctree, "Kconfig", "mainmenu\n");

        // Objtree: pristine copy of src plus build products.
        write(&objtree, "kernel/fork.c", "int x;\n");
        write(&objtree, "Kconfig", "mainmenu\n");
        write(&objtree, "kernel/.fork.o.cmd", FORK);
        write(&objtree, ".built-in.a.cmd", TOP_BUILT_IN);
        write(&objtree, "kernel/fork.o", "ELF");
        write(&objtree, "built-in.a", "!<thin>");
        write(&objtree, "include/config/kernel.release", "6.12.95\n");
        write(&objtree, "include/generated/autoconf.h", "#define X 1\n");
        write(&objtree, ".kbuild-unit-link-env", "declare -x CC=\"gcc\"\n");
        write(&objtree, "kernel/.fork.o.d", "deps");
        write(&objtree, ".tmp_vmlinux1", "elf");
        write(&objtree, "System.map", "map");
        // tools/build sidecar dialect (objtool at defconfig scale): no
        // `savedcmd_` line, absolute target. Skipped, while the built tool
        // itself flows into the snapshot.
        write(
            &objtree,
            "tools/objtool/.builtin-check.o.cmd",
            "cmd_/build/src/tools/objtool/builtin-check.o := gcc -c builtin-check.c\n",
        );
        write(&objtree, "tools/objtool/builtin-check.o", "ELF");
        write(&objtree, "tools/objtool/objtool", "ELF");

        let plan = harvest(&objtree, &srctree, Some(&out)).expect("harvest");

        assert_eq!(plan.kernel_release, "6.12.95");
        let targets: Vec<&str> = plan.cmds.iter().map(|c| c.target.as_str()).collect();
        assert_eq!(targets, ["built-in.a", "kernel/fork.o"]);

        // Unit products, make bookkeeping, and link outputs stay out of the
        // snapshot; kconfig output and the link-env dump flow in.
        assert_eq!(
            plan.generated,
            [
                ".kbuild-unit-link-env",
                "include/config/kernel.release",
                "include/generated/autoconf.h",
                "tools/objtool/objtool",
            ]
        );
        for rel in &plan.generated {
            assert!(out.join(rel).is_file(), "snapshot copy missing {rel}");
        }
    }

    #[test]
    fn handles_symlinks() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let srctree = tmp.path().join("src");
        let objtree = tmp.path().join("obj");
        let out = tmp.path().join("generated");

        // A dir-targeted symlink present identically in src (kbuild's
        // scripts/dtc/include-prefixes/*) stays out of the snapshot.
        write(&srctree, "arch/arc/boot/dts/x.dts", "dts\n");
        write(&objtree, "arch/arc/boot/dts/x.dts", "dts\n");
        std::fs::create_dir_all(srctree.join("scripts/dtc/include-prefixes")).expect("mkdir");
        std::fs::create_dir_all(objtree.join("scripts/dtc/include-prefixes")).expect("mkdir");
        for root in [&srctree, &objtree] {
            std::os::unix::fs::symlink(
                "../../../arch/arc/boot/dts",
                root.join("scripts/dtc/include-prefixes/arc"),
            )
            .expect("symlink fixture");
        }
        // A build-created symlink flows into the snapshot as a symlink.
        std::os::unix::fs::symlink("arch/arc/boot/dts", objtree.join("dts-link"))
            .expect("symlink fixture");
        write(&objtree, "include/config/kernel.release", "6.12.95\n");

        let plan = harvest(&objtree, &srctree, Some(&out)).expect("harvest");
        assert_eq!(
            plan.generated,
            ["dts-link", "include/config/kernel.release"]
        );
        assert!(out.join("dts-link").is_symlink(), "snapshot keeps symlink");
    }

    #[test]
    fn harvests_modules_and_modpost_snapshot_exclusions() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let srctree = tmp.path().join("src");
        let objtree = tmp.path().join("obj");

        write(&srctree, "Kconfig", "mainmenu\n");
        write(&objtree, "Kconfig", "mainmenu\n");
        write(&objtree, "include/config/kernel.release", "6.12.95\n");
        write(
            &objtree,
            "modules.order",
            "drivers/net/dummy.o\nfs/nfs/nfs.o\n",
        );
        // A multi-object module links its <mod>.o from an @-response file;
        // the member list must ride in the plan, and the .mod file itself
        // must stay in the snapshot for the replay to read.
        write(
            &objtree,
            "fs/nfs/.nfs.o.cmd",
            "savedcmd_fs/nfs/nfs.o := ld -m elf_x86_64 -z noexecstack -r -o fs/nfs/nfs.o              @fs/nfs/nfs.mod\n",
        );
        write(
            &objtree,
            "fs/nfs/nfs.mod",
            "fs/nfs/client.o\nfs/nfs/dir.o\n",
        );
        write(&objtree, "fs/nfs/nfs.o", "ELF");
        // Modpost products without their own .cmd: regenerated by the
        // modpost units at replay time, so they stay out of the snapshot.
        write(&objtree, "drivers/net/dummy.mod.c", "mod c\n");
        write(&objtree, ".vmlinux.export.c", "export c\n");
        // Postlink relocation dump: rewritten by the link unit's replay. The
        // realmode dump stays: rmpiggy.o's replay incbins it.
        write(
            &objtree,
            "arch/x86/boot/compressed/vmlinux.relocs",
            "relocs\n",
        );
        write(&objtree, "arch/x86/realmode/rm/realmode.relocs", "relocs\n");

        let plan = harvest(&objtree, &srctree, None).expect("harvest");

        assert_eq!(plan.modules, ["drivers/net/dummy.o", "fs/nfs/nfs.o"]);
        let nfs = plan
            .cmds
            .iter()
            .find(|entry| entry.target == "fs/nfs/nfs.o")
            .expect("nfs.o harvested");
        assert_eq!(
            nfs.at_file_contents
                .get("fs/nfs/nfs.mod")
                .map(String::as_str),
            Some("fs/nfs/client.o\nfs/nfs/dir.o\n")
        );
        assert_eq!(
            plan.generated,
            [
                "arch/x86/realmode/rm/realmode.relocs",
                "fs/nfs/nfs.mod",
                "include/config/kernel.release",
                "modules.order",
            ]
        );
    }

    #[test]
    fn rejects_unrecognized_modules_order_entries() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let srctree = tmp.path().join("src");
        let objtree = tmp.path().join("obj");

        write(&objtree, "include/config/kernel.release", "6.12.95\n");
        write(&objtree, "modules.order", "drivers/net/dummy.ko\n");

        let err = harvest(&objtree, &srctree, None).expect_err("pre-6.5 .ko format must fail");
        assert!(err.to_string().contains("modules.order"));
    }

    #[test]
    fn detects_in_place_modification() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let srctree = tmp.path().join("src");
        let objtree = tmp.path().join("obj");

        write(&srctree, "scripts/link-vmlinux.sh", "#!/bin/sh\n");
        write(
            &objtree,
            "scripts/link-vmlinux.sh",
            "#!/bin/sh\nexport -p\n",
        );
        write(&objtree, "include/config/kernel.release", "6.12.95\n");

        let plan = harvest(&objtree, &srctree, None).expect("harvest");
        assert!(
            plan.generated
                .contains(&"scripts/link-vmlinux.sh".to_owned())
        );
    }
}
