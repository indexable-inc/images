//! Stage 2: classify plan entries into units, wire the dependency graph,
//! prune to what the vmlinux link (and modpost) actually reach, and emit
//! units.nix from the template.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use color_eyre::eyre::{OptionExt as _, bail};

use crate::model::{CmdEntry, Plan, UnitKind};

const UNITS_TEMPLATE: &str = include_str!("../templates/units.nix.in");

pub fn render_units_nix(plan: &Plan, content_addressed: bool) -> color_eyre::Result<String> {
    let mut units: BTreeMap<&str, (&CmdEntry, UnitKind)> = BTreeMap::new();
    for entry in &plan.cmds {
        if let Some(kind) = entry.unit_kind()
            && units.insert(entry.target.as_str(), (entry, kind)).is_some()
        {
            bail!("duplicate unit target {}", entry.target);
        }
    }
    if !units.contains_key("vmlinux") {
        bail!("plan has no vmlinux link command");
    }
    let generated: BTreeSet<&str> = plan.generated.iter().map(String::as_str).collect();

    let mut direct_deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (&target, &(entry, kind)) in &units {
        direct_deps.insert(target, unit_deps(entry, kind, &units, &generated)?);
    }

    // Prune to what the exposed roots reach: vdso/realmode intermediates are
    // consumed at plan time and never feed the link.
    let mut reachable: BTreeSet<&str> = BTreeSet::new();
    let mut queue: Vec<&str> = ["vmlinux", "vmlinux.symvers"]
        .into_iter()
        .filter(|root| units.contains_key(root))
        .collect();
    while let Some(target) = queue.pop() {
        if !reachable.insert(target) {
            continue;
        }
        queue.extend(&direct_deps[target]);
    }
    let pruned: Vec<&str> = units
        .keys()
        .copied()
        .filter(|target| !reachable.contains(target))
        .collect();

    // Thin archives resolve members at read time, so a unit's build tree
    // needs its full transitive dep closure, not just direct deps.
    let mut closures: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for &target in &reachable {
        closure_of(target, &direct_deps, &mut closures)?;
    }

    let mut entries = String::new();
    for &target in &reachable {
        let (entry, kind) = units[target];
        render_unit(&mut entries, entry, kind, &closures[target]);
    }

    let mut pruned_list = String::new();
    for target in &pruned {
        writeln!(pruned_list, "    {}", nix_string(target)).expect("write to string");
    }

    fill_template(
        UNITS_TEMPLATE,
        &[
            ("kernel_release", nix_string(&plan.kernel_release)),
            ("content_addressed", content_addressed.to_string()),
            ("unit_entries", entries),
            ("pruned_targets", pruned_list),
        ],
    )
}

/// Direct unit-level dependencies of one saved command.
fn unit_deps<'plan>(
    entry: &'plan CmdEntry,
    kind: UnitKind,
    units: &BTreeMap<&'plan str, (&'plan CmdEntry, UnitKind)>,
    generated: &BTreeSet<&str>,
) -> color_eyre::Result<BTreeSet<&'plan str>> {
    let mut deps = BTreeSet::new();
    match kind {
        UnitKind::Compile => {
            // Tracked prerequisites are almost always sources and headers; a
            // unit-built prerequisite (another unit's output) becomes a dep.
            for dep in entry.source.iter().chain(&entry.deps) {
                if let Some((key, _)) = units.get_key_value(dep.as_str()) {
                    deps.insert(*key);
                }
            }
        }
        UnitKind::Archive | UnitKind::ObjectAggregate | UnitKind::Modpost => {
            // Member lists live in the command line itself (`ar`/`ld`/modpost
            // argv, or the `printf ... | xargs ar` form for the top-level
            // archive). Token-scan for object and archive operands only;
            // interpreting any more shell than that is out of scope.
            // Nested archives name members relative to their directory, with
            // the prefix in the printf format: `printf "arch/x86/%s " entry/...`;
            // the pipe to xargs ends the member list.
            let mut printf_prefix: Option<&str> = None;
            for token in entry.cmd.split_whitespace() {
                if token == "|" {
                    printf_prefix = None;
                    continue;
                }
                let unquoted = token.trim_matches('"');
                if let Some(prefix) = unquoted.strip_suffix("%s") {
                    printf_prefix = Some(prefix);
                    continue;
                }
                let cleaned = unquoted.trim_end_matches(';');
                let cleaned = printf_prefix.map_or(Cow::Borrowed(cleaned), |prefix| {
                    Cow::Owned(format!("{prefix}{cleaned}"))
                });
                let cleaned = cleaned.strip_prefix("./").unwrap_or(&cleaned);
                if cleaned == entry.target || !(cleaned.ends_with(".o") || cleaned.ends_with(".a"))
                {
                    continue;
                }
                if let Some((key, _)) = units.get_key_value(cleaned) {
                    deps.insert(*key);
                } else if !generated.contains(cleaned) {
                    bail!(
                        "unit {} references {cleaned}, which is neither a unit nor in \
                         the generated snapshot",
                        entry.target
                    );
                }
            }
        }
        UnitKind::Link => {
            // link-vmlinux.sh reads no argv member list; since 5.16 it links
            // the prelinked vmlinux.o (plus generated inputs: the linker
            // script and init/version-timestamp.c against snapshot headers).
            let (key, _) = units
                .get_key_value("vmlinux.o")
                .ok_or_eyre("vmlinux link present but no vmlinux.o aggregate")?;
            deps.insert(*key);
        }
    }
    Ok(deps)
}

fn closure_of<'plan>(
    target: &'plan str,
    direct_deps: &BTreeMap<&'plan str, BTreeSet<&'plan str>>,
    closures: &mut BTreeMap<&'plan str, BTreeSet<&'plan str>>,
) -> color_eyre::Result<()> {
    if closures.contains_key(target) {
        return Ok(());
    }
    // Iterative DFS with an explicit in-progress set for cycle detection.
    let mut stack = vec![(target, false)];
    let mut in_progress: BTreeSet<&str> = BTreeSet::new();
    while let Some((current, children_done)) = stack.pop() {
        if children_done {
            let mut closure = BTreeSet::new();
            for &dep in &direct_deps[current] {
                closure.insert(dep);
                closure.extend(&closures[dep]);
            }
            closures.insert(current, closure);
            in_progress.remove(current);
            continue;
        }
        if closures.contains_key(current) {
            continue;
        }
        if !in_progress.insert(current) {
            bail!("dependency cycle through unit {current}");
        }
        stack.push((current, true));
        for &dep in &direct_deps[current] {
            if in_progress.contains(dep) {
                bail!("dependency cycle through unit {dep}");
            }
            stack.push((dep, false));
        }
    }
    Ok(())
}

fn render_unit(out: &mut String, entry: &CmdEntry, kind: UnitKind, closure: &BTreeSet<&str>) {
    let target = entry.target.as_str();
    writeln!(out, "    {} = mkUnit {{", nix_string(target)).expect("write to string");
    writeln!(out, "      pname = {};", nix_string(&pname(target))).expect("write to string");
    writeln!(out, "      target = {};", nix_string(target)).expect("write to string");
    writeln!(out, "      kbuildCmd = {};", nix_string(&entry.cmd)).expect("write to string");
    if !closure.is_empty() {
        writeln!(out, "      depUnits = [").expect("write to string");
        for dep in closure {
            writeln!(out, "        units.{}", nix_string(dep)).expect("write to string");
        }
        writeln!(out, "      ];").expect("write to string");
    }
    if kind == UnitKind::Link {
        // The link also emits System.map; keep it next to vmlinux.
        writeln!(out, "      installFiles = [").expect("write to string");
        writeln!(out, "        {}", nix_string(target)).expect("write to string");
        writeln!(out, "        \"System.map\"").expect("write to string");
        writeln!(out, "      ];").expect("write to string");
        writeln!(out, "      sourceLinkEnv = true;").expect("write to string");
    }
    writeln!(out, "    }};").expect("write to string");
}

/// Derivation-name-safe pname for a unit target.
fn pname(target: &str) -> String {
    let sanitized: String = target
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("kbuild-{sanitized}")
}

/// Quote a string as a Nix double-quoted literal.
fn nix_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push_str("\\${");
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn fill_template(template: &str, slots: &[(&str, String)]) -> color_eyre::Result<String> {
    let mut out = template.to_owned();
    for (name, value) in slots {
        let marker = format!("{{{{ {name} }}}}");
        if !out.contains(&marker) {
            bail!("template slot {marker} missing from units.nix.in");
        }
        out = out.replace(&marker, value);
    }
    if let Some(idx) = out.find("{{") {
        let tail: String = out[idx..].chars().take(40).collect();
        bail!("unfilled template slot at {tail:?}");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(target: &str, cmd: &str, source: Option<&str>, deps: &[&str]) -> CmdEntry {
        CmdEntry {
            target: target.to_owned(),
            cmd: cmd.to_owned(),
            source: source.map(str::to_owned),
            deps: deps.iter().map(|&d| d.to_owned()).collect(),
            config_deps: Vec::new(),
        }
    }

    fn sample_plan() -> Plan {
        Plan {
            kernel_release: "6.12.95".to_owned(),
            cmds: vec![
                entry(
                    "kernel/fork.o",
                    "gcc -c -o kernel/fork.o kernel/fork.c",
                    Some("kernel/fork.c"),
                    &["include/linux/sched.h"],
                ),
                entry(
                    "kernel/built-in.a",
                    "rm -f kernel/built-in.a; ar cDPrST kernel/built-in.a kernel/fork.o",
                    None,
                    &[],
                ),
                entry(
                    "built-in.a",
                    "rm -f built-in.a;  printf \"./%s \" kernel/built-in.a | xargs ar cDPrST built-in.a",
                    None,
                    &[],
                ),
                entry(
                    "vmlinux.a",
                    "rm -f vmlinux.a; ar cDPrST vmlinux.a ./built-in.a",
                    None,
                    &[],
                ),
                entry(
                    "vmlinux.o",
                    "ld -r -o vmlinux.o --whole-archive vmlinux.a --no-whole-archive",
                    None,
                    &[],
                ),
                entry(
                    "vmlinux",
                    "scripts/link-vmlinux.sh \"ld\" \"-m elf_i386\" \"--build-id=sha1\"; make -f ./arch/x86/Makefile.postlink vmlinux",
                    Some("scripts/link-vmlinux.sh"),
                    &[],
                ),
                entry(
                    "vmlinux.symvers",
                    "scripts/mod/modpost -E -o vmlinux.symvers vmlinux.o",
                    None,
                    &[],
                ),
                // Unreachable from the link: pruned.
                entry(
                    "arch/x86/entry/vdso/vdso32/note.o",
                    "gcc -c -o arch/x86/entry/vdso/vdso32/note.o arch/x86/entry/vdso/vdso32/note.S",
                    Some("arch/x86/entry/vdso/vdso32/note.S"),
                    &[],
                ),
                // Host tool object: not a unit at all.
                entry(
                    "scripts/mod/empty.o",
                    "gcc -c -o scripts/mod/empty.o scripts/mod/empty.c",
                    Some("scripts/mod/empty.c"),
                    &[],
                ),
            ],
            generated: vec!["include/generated/autoconf.h".to_owned()],
        }
    }

    #[test]
    fn renders_reachable_units_and_prunes_the_rest() {
        let rendered = render_units_nix(&sample_plan(), true).expect("render");

        assert!(rendered.contains("\"kernel/fork.o\" = mkUnit {"));
        assert!(rendered.contains("\"vmlinux\" = mkUnit {"));
        assert!(rendered.contains("\"vmlinux.symvers\" = mkUnit {"));
        assert!(!rendered.contains("\"arch/x86/entry/vdso/vdso32/note.o\" = mkUnit"));
        assert!(rendered.contains("\"arch/x86/entry/vdso/vdso32/note.o\"\n"));
        assert!(!rendered.contains("scripts/mod/empty.o"));
        assert!(rendered.contains("contentAddressed = true;"));
        assert!(rendered.contains("sourceLinkEnv = true;"));
        assert!(rendered.contains("\"System.map\""));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn wires_transitive_closures_through_thin_archives() {
        let rendered = render_units_nix(&sample_plan(), false).expect("render");

        // vmlinux.o must see the archives AND their members (thin archives
        // resolve member paths at read time).
        let aggregate = rendered
            .split("\"vmlinux.o\" = mkUnit {")
            .nth(1)
            .expect("vmlinux.o unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        for dep in [
            "units.\"vmlinux.a\"",
            "units.\"built-in.a\"",
            "units.\"kernel/built-in.a\"",
            "units.\"kernel/fork.o\"",
        ] {
            assert!(aggregate.contains(dep), "vmlinux.o closure missing {dep}");
        }
        assert!(rendered.contains("contentAddressed = false;"));
    }

    #[test]
    fn resolves_printf_prefixed_archive_members() {
        let mut plan = sample_plan();
        // Nested archives carry the directory prefix in the printf format
        // (real 6.12 form); the xargs target after the pipe must not get it.
        plan.cmds.push(entry(
            "arch/x86/entry/built-in.a",
            "rm -f arch/x86/entry/built-in.a; ar cDPrST arch/x86/entry/built-in.a",
            None,
            &[],
        ));
        plan.cmds.push(entry(
            "arch/x86/built-in.a",
            "rm -f arch/x86/built-in.a;  printf \"arch/x86/%s \" entry/built-in.a | \
             xargs ar cDPrST arch/x86/built-in.a",
            None,
            &[],
        ));
        plan.cmds[2].cmd = "rm -f built-in.a;  printf \"./%s \" kernel/built-in.a \
             arch/x86/built-in.a | xargs ar cDPrST built-in.a"
            .to_owned();

        let rendered = render_units_nix(&plan, false).expect("render");
        let top = rendered
            .split("\"built-in.a\" = mkUnit {")
            .nth(1)
            .expect("built-in.a unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        assert!(top.contains("units.\"arch/x86/built-in.a\""));
        assert!(top.contains("units.\"arch/x86/entry/built-in.a\""));
    }

    #[test]
    fn rejects_unknown_archive_members() {
        let mut plan = sample_plan();
        plan.cmds.push(entry(
            "lib/lib.a",
            "rm -f lib/lib.a; ar cDPrsT lib/lib.a lib/missing.o",
            None,
            &[],
        ));
        // Reach it from the aggregate so it is not pruned before validation.
        plan.cmds[4].cmd =
            "ld -r -o vmlinux.o --whole-archive vmlinux.a --no-whole-archive lib/lib.a".to_owned();

        let err = render_units_nix(&plan, false).expect_err("unknown member must fail");
        assert!(err.to_string().contains("lib/missing.o"));
    }

    #[test]
    fn requires_a_vmlinux_link() {
        let plan = Plan {
            kernel_release: "6.12.95".to_owned(),
            cmds: vec![],
            generated: vec![],
        };
        let err = render_units_nix(&plan, false).expect_err("missing link must fail");
        assert!(err.to_string().contains("no vmlinux link command"));
    }

    #[test]
    fn nix_string_escapes_interpolation() {
        assert_eq!(nix_string(r#"a "b" ${c} \d"#), r#""a \"b\" \${c} \\d""#);
    }
}
