//! Stage 2: classify plan entries into units, wire the dependency graph,
//! prune to what the vmlinux link (and modpost) actually reach, and emit
//! units.nix from the template.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use color_eyre::eyre::{OptionExt as _, bail, eyre};

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

    let provided_files = modpost_provided_files(plan, &units)?;
    // Reverse view for installFiles: what each providing unit must install
    // beyond its own target.
    let mut provides: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (rel, unit) in &provided_files {
        provides.entry(*unit).or_default().push(rel.as_str());
    }

    let mut direct_deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (&target, &(entry, kind)) in &units {
        direct_deps.insert(
            target,
            unit_deps(
                entry,
                kind,
                &units,
                &generated,
                &provided_files,
                &plan.modules,
            )?,
        );
    }

    let reachable = reachable_units(&units, &direct_deps);
    let pruned: Vec<&str> = units
        .keys()
        .copied()
        .filter(|target| !reachable.contains(target))
        .collect();

    // Thin archives resolve members by path at read time, so a consumer's
    // build tree must materialize the archive's members, recursively through
    // nested thin archives. Everything else a saved command reads (`ld -r`
    // outputs, compiled objects, modpost dumps) is self-contained, so direct
    // deps suffice. Carrying the full transitive closure instead re-linked
    // every `.ko` after any single-module body edit: the dep list rode
    // through `Module.symvers` to every module object (#3413).
    let mut expansions: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut closures: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut dep_sets: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for &target in &reachable {
        // The vmlinux link is the one exception to the trim:
        // scripts/link-vmlinux.sh links the KBUILD_VMLINUX_OBJS/LIBS
        // archives directly (vmlinux.a, lib/lib.a, ... -- an env-driven
        // list no argv or .cmd records; x86_64 defconfig under IBT links
        // vmlinux.o instead), so its tree keeps the full transitive
        // closure, exactly what every unit carried before the trim.
        let deps = if units[target].1 == UnitKind::Link {
            full_closure(target, &direct_deps, &mut closures)?;
            closures[target].clone()
        } else {
            unit_dep_set(target, &units, &direct_deps, &mut expansions)?
        };
        dep_sets.insert(target, deps);
    }

    let srcarch = detect_srcarch(&units)?;

    let mut scopes: BTreeMap<&str, SourceScope> = BTreeMap::new();
    for &target in &reachable {
        let (entry, kind) = units[target];
        scopes.insert(
            target,
            source_scope(entry, kind, &units, &generated, &provided_files, &srcarch)?,
        );
    }
    let farm: BTreeSet<&str> = scopes
        .values()
        .flat_map(|scope| scope.files.iter().map(String::as_str))
        .collect();

    let mut entries = String::new();
    for &target in &reachable {
        let (entry, kind) = units[target];
        render_unit(
            &mut entries,
            entry,
            kind,
            &dep_sets[target],
            &scopes[target],
            provides.get(target).map_or(&[][..], Vec::as_slice),
        );
    }

    let mut farm_entries = String::new();
    for rel in &farm {
        writeln!(
            farm_entries,
            "    {} = srcFile {} {};",
            nix_string(rel),
            nix_string(&store_name(rel)),
            nix_string(rel)
        )
        .expect("write to string");
    }

    let mut pruned_list = String::new();
    for target in &pruned {
        writeln!(pruned_list, "    {}", nix_string(target)).expect("write to string");
    }

    let (module_symvers, module_entries, all_unit_entries) = module_export_slots(&reachable);

    fill_template(
        UNITS_TEMPLATE,
        &[
            ("kernel_release", nix_string(&plan.kernel_release)),
            ("content_addressed", content_addressed.to_string()),
            ("unit_entries", entries),
            ("source_farm_entries", farm_entries),
            ("pruned_targets", pruned_list),
            ("module_symvers", module_symvers),
            ("module_entries", module_entries),
            ("all_unit_entries", all_unit_entries),
        ],
    )
}

/// Prune to what the exposed roots reach: vmlinux, the modpost dumps, and
/// every final `.ko`; vdso/realmode intermediates are consumed at plan time
/// and never feed the link.
fn reachable_units<'plan>(
    units: &BTreeMap<&'plan str, (&'plan CmdEntry, UnitKind)>,
    direct_deps: &BTreeMap<&'plan str, BTreeSet<&'plan str>>,
) -> BTreeSet<&'plan str> {
    let mut reachable: BTreeSet<&str> = BTreeSet::new();
    let mut queue: Vec<&str> = ["vmlinux", "vmlinux.symvers", "Module.symvers"]
        .into_iter()
        .filter(|root| units.contains_key(root))
        .chain(
            units
                .keys()
                .copied()
                .filter(|target| target.ends_with(".ko")),
        )
        .collect();
    while let Some(target) = queue.pop() {
        if !reachable.insert(target) {
            continue;
        }
        queue.extend(&direct_deps[target]);
    }
    reachable
}

/// Files a modpost unit regenerates at replay time and therefore provides to
/// its consumers (they exist in neither srctree nor snapshot), keyed by
/// objtree-relative path: `vmlinux.symvers` emits `.vmlinux.export.c`,
/// `Module.symvers` emits every module's `<mod>.mod.c`.
fn modpost_provided_files<'plan>(
    plan: &'plan Plan,
    units: &BTreeMap<&'plan str, (&'plan CmdEntry, UnitKind)>,
) -> color_eyre::Result<BTreeMap<String, &'plan str>> {
    let mut provided_files: BTreeMap<String, &str> = BTreeMap::new();
    if units.contains_key(".vmlinux.export.o") {
        let (modpost, _) = units
            .get_key_value("vmlinux.symvers")
            .ok_or_eyre(".vmlinux.export.o present but no vmlinux.symvers modpost")?;
        provided_files.insert(".vmlinux.export.c".to_owned(), *modpost);
    }
    if let Some((modpost_modules, _)) = units.get_key_value("Module.symvers") {
        for base in &plan.modules {
            let stem = base
                .strip_suffix(".o")
                .ok_or_else(|| eyre!("modules.order entry {base:?} is not an .o path"))?;
            provided_files.insert(format!("{stem}.mod.c"), *modpost_modules);
        }
    }
    Ok(provided_files)
}

/// Render the module-facing template slots from the reachable unit set: the
/// `moduleSymvers` export, the `.ko` link-farm entries, and the all-units
/// aggregation entries.
fn module_export_slots(reachable: &BTreeSet<&str>) -> (String, String, String) {
    let module_symvers = if reachable.contains("Module.symvers") {
        "units.\"Module.symvers\"".to_owned()
    } else {
        "null".to_owned()
    };
    let mut module_entries = String::new();
    let mut all_unit_entries = String::new();
    for &target in reachable {
        let quoted = nix_string(target);
        if target.ends_with(".ko") {
            writeln!(
                module_entries,
                "    {{ name = {quoted}; path = \"${{units.{quoted}}}/\" + {quoted}; }}"
            )
            .expect("write to string");
        }
        writeln!(
            all_unit_entries,
            "    {{ name = {quoted}; path = units.{quoted}; }}"
        )
        .expect("write to string");
    }
    (module_symvers, module_entries, all_unit_entries)
}

/// Direct unit-level dependencies of one saved command.
fn unit_deps<'plan>(
    entry: &'plan CmdEntry,
    kind: UnitKind,
    units: &BTreeMap<&'plan str, (&'plan CmdEntry, UnitKind)>,
    generated: &BTreeSet<&str>,
    provided_files: &BTreeMap<String, &'plan str>,
    modules: &'plan [String],
) -> color_eyre::Result<BTreeSet<&'plan str>> {
    let mut deps = BTreeSet::new();
    match kind {
        UnitKind::Compile => {
            // Tracked prerequisites are almost always sources and headers; a
            // unit-built prerequisite (another unit's output) becomes a dep,
            // and a modpost-provided source (<mod>.mod.c, .vmlinux.export.c)
            // wires in the modpost unit that regenerates it.
            for dep in entry.source.iter().chain(&entry.deps) {
                if let Some((key, _)) = units.get_key_value(dep.as_str()) {
                    deps.insert(*key);
                } else if !dep.starts_with('/')
                    && let Some(provider) = provided_files.get(normalize_rel(dep)?.as_str())
                {
                    deps.insert(*provider);
                }
            }
        }
        UnitKind::Archive
        | UnitKind::ObjectAggregate
        | UnitKind::Modpost
        | UnitKind::ModpostModules
        | UnitKind::ModuleLink
        | UnitKind::Command => {
            // Member lists live in the command line itself (`ar`/`ld`/modpost
            // argv, the `printf ... | xargs ar` form for the top-level
            // archive, a Command unit's strip/objcopy input operands, or the
            // `.ko` link's object list); interpreting any more shell than
            // that is out of scope.
            // Nested archives name members relative to their directory, with
            // the prefix in the printf format: `printf "arch/x86/%s " entry/...`;
            // the pipe to xargs ends the member list. A multi-object module
            // links `@<mod>.mod`, whose harvested contents expand like argv.
            let mut printf_prefix: Option<&str> = None;
            for token in entry.cmd.split_whitespace() {
                if token == "|" {
                    printf_prefix = None;
                    continue;
                }
                let unquoted = token.trim_matches('"');
                if let Some(at_ref) = unquoted.strip_prefix('@') {
                    let contents = entry.at_file_contents.get(at_ref).ok_or_else(|| {
                        eyre!(
                            "unit {} references response file @{at_ref}, which harvest \
                             did not capture",
                            entry.target
                        )
                    })?;
                    for member in contents.split_whitespace() {
                        resolve_member(member, entry, units, generated, &mut deps)?;
                    }
                    continue;
                }
                if let Some(prefix) = unquoted.strip_suffix("%s") {
                    printf_prefix = Some(prefix);
                    continue;
                }
                let cleaned = unquoted.trim_end_matches(';');
                let cleaned = printf_prefix.map_or(Cow::Borrowed(cleaned), |prefix| {
                    Cow::Owned(format!("{prefix}{cleaned}"))
                });
                resolve_member(&cleaned, entry, units, generated, &mut deps)?;
            }
            if kind == UnitKind::ModpostModules {
                // The module member list lives in modules.order (passed as
                // `-T modules.order`), not in argv; vmlinux.o still rides in
                // argv and resolves through the token scan above.
                for base in modules {
                    let (key, _) = units.get_key_value(base.as_str()).ok_or_else(|| {
                        eyre!("modules.order entry {base} has no corresponding unit")
                    })?;
                    deps.insert(*key);
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
            // On CONFIG_MODULES builds the script also links the modpost-built
            // ksymtab object into vmlinux.
            if let Some((key, _)) = units.get_key_value(".vmlinux.export.o") {
                deps.insert(*key);
            }
        }
    }
    Ok(deps)
}

/// Resolve one member token (or response-file line) of an aggregation
/// command to a unit dep, tolerating non-member tokens and bailing on a
/// member that nothing accounts for.
fn resolve_member<'plan>(
    raw: &str,
    entry: &CmdEntry,
    units: &BTreeMap<&'plan str, (&'plan CmdEntry, UnitKind)>,
    generated: &BTreeSet<&str>,
    deps: &mut BTreeSet<&'plan str>,
) -> color_eyre::Result<()> {
    let cleaned = raw.strip_prefix("./").unwrap_or(raw);
    if cleaned == entry.target || !(cleaned.ends_with(".o") || cleaned.ends_with(".a")) {
        return Ok(());
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
    Ok(())
}

/// The dep units whose outputs `target`'s build tree materializes: its
/// direct deps, plus each thin-archive dep's member expansion.
fn unit_dep_set<'plan>(
    target: &'plan str,
    units: &BTreeMap<&'plan str, (&'plan CmdEntry, UnitKind)>,
    direct_deps: &BTreeMap<&'plan str, BTreeSet<&'plan str>>,
    expansions: &mut BTreeMap<&'plan str, BTreeSet<&'plan str>>,
) -> color_eyre::Result<BTreeSet<&'plan str>> {
    let mut deps = BTreeSet::new();
    for &dep in &direct_deps[target] {
        deps.insert(dep);
        if units[dep].1 == UnitKind::Archive {
            archive_expansion(dep, units, direct_deps, expansions)?;
            deps.extend(&expansions[dep]);
        }
    }
    Ok(deps)
}

/// Memoized full transitive dep closure (the vmlinux link unit's dep set).
/// Iterative DFS with an explicit in-progress set for cycle detection.
fn full_closure<'plan>(
    target: &'plan str,
    direct_deps: &BTreeMap<&'plan str, BTreeSet<&'plan str>>,
    closures: &mut BTreeMap<&'plan str, BTreeSet<&'plan str>>,
) -> color_eyre::Result<()> {
    if closures.contains_key(target) {
        return Ok(());
    }
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

/// Memoized member set of one thin archive: its direct deps plus, through
/// nested thin archives, theirs.
fn archive_expansion<'plan>(
    target: &'plan str,
    units: &BTreeMap<&'plan str, (&'plan CmdEntry, UnitKind)>,
    direct_deps: &BTreeMap<&'plan str, BTreeSet<&'plan str>>,
    expansions: &mut BTreeMap<&'plan str, BTreeSet<&'plan str>>,
) -> color_eyre::Result<()> {
    if expansions.contains_key(target) {
        return Ok(());
    }
    // Iterative DFS with an explicit in-progress set for cycle detection.
    let mut stack = vec![(target, false)];
    let mut in_progress: BTreeSet<&str> = BTreeSet::new();
    while let Some((current, children_done)) = stack.pop() {
        if children_done {
            let mut expansion = BTreeSet::new();
            for &dep in &direct_deps[current] {
                expansion.insert(dep);
                if units[dep].1 == UnitKind::Archive {
                    expansion.extend(&expansions[dep]);
                }
            }
            expansions.insert(current, expansion);
            in_progress.remove(current);
            continue;
        }
        if expansions.contains_key(current) {
            continue;
        }
        if !in_progress.insert(current) {
            bail!("dependency cycle through unit {current}");
        }
        stack.push((current, true));
        for &dep in &direct_deps[current] {
            if units[dep].1 != UnitKind::Archive || expansions.contains_key(dep) {
                continue;
            }
            if in_progress.contains(dep) {
                bail!("dependency cycle through unit {dep}");
            }
            stack.push((dep, false));
        }
    }
    Ok(())
}

/// The srctree inputs a unit's replay is allowed to see (#3412): tracked
/// files resolved through the per-file source farm, plus directory prefixes
/// for the script-driven link whose reads no .cmd records.
struct SourceScope {
    files: BTreeSet<String>,
    dirs: Vec<String>,
}

/// Lexically resolve `.` and `..` in a srctree-relative path. kbuild records
/// cpp -MD prerequisites verbatim, so one header can appear under several
/// spellings (e.g. `arch/x86/mm/../include/asm/trace/exceptions.h`) and
/// collide in the replay symlink farm unless canonicalized.
fn normalize_rel(rel: &str) -> color_eyre::Result<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    bail!("source path {rel:?} escapes the source tree");
                }
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        bail!("source path {rel:?} normalizes to nothing");
    }
    Ok(parts.join("/"))
}

/// Srctree files a saved command reads through `grep -f` (member-list
/// filters like `scripts/head-object-list.txt`). Such reads leave no .cmd
/// dep, so the source scope must recover them from the command text. Only a
/// `-f` inside a grep invocation counts: `rm -f`/`ar`-flag lookalikes in the
/// same command line stay ignored, and a pipe or statement end closes the
/// grep. Absolute paths arrive store-resolved and need no farm entry.
fn grep_file_reads(cmd: &str) -> color_eyre::Result<Vec<String>> {
    let mut reads = Vec::new();
    let mut in_grep = false;
    let mut next_is_file = false;
    for raw in cmd.split_whitespace() {
        let token = raw.trim_matches('"');
        if next_is_file {
            next_is_file = false;
            in_grep = false;
            // Command substitution wraps the pipeline, so the file operand
            // can carry the closing `)`.
            let path = token.trim_end_matches([')', ';']);
            if !path.starts_with('/') {
                reads.push(normalize_rel(path)?);
            }
            continue;
        }
        if token == "|" || token == "&&" || token == "||" || token.ends_with(';') {
            in_grep = false;
            continue;
        }
        match token {
            "grep" => in_grep = true,
            "-f" if in_grep => next_is_file = true,
            _ => {}
        }
    }
    Ok(reads)
}

/// The single arch the plan builds for, from `arch/<srcarch>/` unit targets.
fn detect_srcarch(units: &BTreeMap<&str, (&CmdEntry, UnitKind)>) -> color_eyre::Result<String> {
    let arches: BTreeSet<&str> = units
        .keys()
        .filter_map(|target| target.strip_prefix("arch/"))
        .filter_map(|rest| rest.split('/').next())
        .collect();
    match arches.len() {
        1 => Ok((*arches.first().expect("len checked")).to_owned()),
        0 => bail!("no arch/<srcarch>/ unit targets in plan"),
        _ => bail!("multiple srcarch candidates in plan: {arches:?}"),
    }
}

fn source_scope(
    entry: &CmdEntry,
    kind: UnitKind,
    units: &BTreeMap<&str, (&CmdEntry, UnitKind)>,
    generated: &BTreeSet<&str>,
    provided_files: &BTreeMap<String, &str>,
    srcarch: &str,
) -> color_eyre::Result<SourceScope> {
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut dirs: Vec<String> = Vec::new();
    match kind {
        UnitKind::Compile => {
            for dep in entry.source.iter().chain(&entry.deps) {
                if dep.starts_with('/') {
                    // Toolchain-owned (store) headers arrive via the compiler.
                    continue;
                }
                if dep.chars().any(char::is_whitespace) {
                    bail!("whitespace in source path {dep:?} of {}", entry.target);
                }
                let rel = normalize_rel(dep)?;
                if units.contains_key(rel.as_str())
                    || generated.contains(rel.as_str())
                    || provided_files.contains_key(rel.as_str())
                {
                    // Dep unit outputs, snapshot members, and modpost-provided
                    // sources overlay the tree; none exists in srctree.
                    continue;
                }
                files.insert(rel);
            }
        }
        UnitKind::Link => {
            // link-vmlinux.sh (snapshot member, sed-patched at plan time)
            // shells back into make and compiles sources with no .cmd of
            // their own; give it the Makefile machinery and header trees.
            files.insert("Makefile".to_owned());
            // The script rebuilds the timestamp object through `${MAKE} -f
            // ${srctree}/scripts/Makefile.build obj=init
            // init/version-timestamp.o`, and Makefile.build includes the obj
            // directory's kbuild file; init/ is the only subdir the script
            // descends into.
            files.insert("init/Makefile".to_owned());
            files.insert("init/version-timestamp.c".to_owned());
            if let Some(postlink) = entry
                .cmd
                .split_whitespace()
                .find(|token| token.ends_with("Makefile.postlink"))
            {
                files.insert(normalize_rel(postlink)?);
            }
            dirs = vec![
                format!("arch/{srcarch}/include"),
                "include".to_owned(),
                "scripts".to_owned(),
            ];
        }
        // Aggregation replays dep unit outputs plus snapshot tools, but a
        // saved command can still read srctree files no .cmd dep records:
        // cmd_ar_vmlinux.a reorders boot head objects to the archive front
        // through `grep -F -f $(srctree)/scripts/head-object-list.txt`, and
        // without that file the grep fails, the reorder silently no-ops, and
        // the linked vmlinux diverges from the monolithic reference.
        UnitKind::Archive
        | UnitKind::ObjectAggregate
        | UnitKind::Modpost
        | UnitKind::ModpostModules
        | UnitKind::ModuleLink
        | UnitKind::Command => {
            for rel in grep_file_reads(&entry.cmd)? {
                if !units.contains_key(rel.as_str()) && !generated.contains(rel.as_str()) {
                    files.insert(rel);
                }
            }
        }
    }
    Ok(SourceScope { files, dirs })
}

/// Store-path name for a farm entry: sanitized basename, never dot-leading.
fn store_name(rel: &str) -> String {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    let sanitized: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '=' | '?') {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("kbuild-src-{sanitized}")
}

fn render_unit(
    out: &mut String,
    entry: &CmdEntry,
    kind: UnitKind,
    dep_set: &BTreeSet<&str>,
    scope: &SourceScope,
    provides: &[&str],
) {
    let target = entry.target.as_str();
    writeln!(out, "    {} = mkUnit {{", nix_string(target)).expect("write to string");
    writeln!(out, "      pname = {};", nix_string(&pname(target))).expect("write to string");
    writeln!(out, "      target = {};", nix_string(target)).expect("write to string");
    writeln!(out, "      kbuildCmd = {};", nix_string(&entry.cmd)).expect("write to string");
    if !dep_set.is_empty() {
        writeln!(out, "      depUnits = [").expect("write to string");
        for dep in dep_set {
            writeln!(out, "        units.{}", nix_string(dep)).expect("write to string");
        }
        writeln!(out, "      ];").expect("write to string");
    }
    if !scope.files.is_empty() {
        writeln!(out, "      sourceFiles = [").expect("write to string");
        for rel in &scope.files {
            writeln!(out, "        {}", nix_string(rel)).expect("write to string");
        }
        writeln!(out, "      ];").expect("write to string");
    }
    if !scope.dirs.is_empty() {
        writeln!(out, "      sourceDirs = [").expect("write to string");
        for dir in &scope.dirs {
            writeln!(out, "        {}", nix_string(dir)).expect("write to string");
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
    } else if !provides.is_empty() {
        // A modpost unit installs the sources it regenerates for its
        // consumers alongside its own target.
        writeln!(out, "      installFiles = [").expect("write to string");
        writeln!(out, "        {}", nix_string(target)).expect("write to string");
        for rel in provides {
            writeln!(out, "        {}", nix_string(rel)).expect("write to string");
        }
        writeln!(out, "      ];").expect("write to string");
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
            at_file_contents: std::collections::BTreeMap::new(),
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
                    "rm -f vmlinux.a; ar cDPrST vmlinux.a ./built-in.a; ar mPiT $(ar t \
                     vmlinux.a | sed -n 1p) vmlinux.a $(ar t vmlinux.a | grep -F -f \
                     ./scripts/head-object-list.txt)",
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
            modules: Vec::new(),
        }
    }

    /// `sample_plan` extended the way a `CONFIG_MODULES=y` defconfig build
    /// extends it: one single-object module, one multi-object module (linked
    /// through an @-response file), the shared `.module-common.o`, the
    /// ksymtab export chain, and the `make modules` modpost pass.
    fn module_plan() -> Plan {
        let mut plan = sample_plan();
        plan.modules = vec!["drivers/net/dummy.o".to_owned(), "fs/nfs/nfs.o".to_owned()];
        plan.generated.push("fs/nfs/nfs.mod".to_owned());
        plan.generated.push("modules.order".to_owned());
        plan.generated.push("scripts/module.lds".to_owned());
        plan.cmds.push(entry(
            ".vmlinux.export.o",
            "gcc -c -o .vmlinux.export.o .vmlinux.export.c",
            Some(".vmlinux.export.c"),
            &[".vmlinux.export.c", "include/linux/export-internal.h"],
        ));
        plan.cmds.push(entry(
            ".module-common.o",
            "gcc -c -o .module-common.o scripts/module-common.c",
            Some("scripts/module-common.c"),
            &["scripts/module-common.c"],
        ));
        plan.cmds.push(entry(
            "drivers/net/dummy.o",
            "gcc -c -o drivers/net/dummy.o drivers/net/dummy.c",
            Some("drivers/net/dummy.c"),
            &[],
        ));
        plan.cmds.push(entry(
            "fs/nfs/client.o",
            "gcc -c -o fs/nfs/client.o fs/nfs/client.c",
            Some("fs/nfs/client.c"),
            &[],
        ));
        let mut nfs = entry(
            "fs/nfs/nfs.o",
            "ld -m elf_x86_64 -z noexecstack -r -o fs/nfs/nfs.o @fs/nfs/nfs.mod",
            None,
            &[],
        );
        nfs.at_file_contents
            .insert("fs/nfs/nfs.mod".to_owned(), "fs/nfs/client.o\n".to_owned());
        plan.cmds.push(nfs);
        plan.cmds.push(entry(
            "Module.symvers",
            "scripts/mod/modpost -M -E -o Module.symvers -T modules.order vmlinux.o",
            None,
            &[],
        ));
        for module in ["drivers/net/dummy", "fs/nfs/nfs"] {
            plan.cmds.push(entry(
                &format!("{module}.mod.o"),
                &format!("gcc -c -o {module}.mod.o {module}.mod.c"),
                Some(&format!("{module}.mod.c")),
                &[&format!("{module}.mod.c")],
            ));
            plan.cmds.push(entry(
                &format!("{module}.ko"),
                &format!(
                    "ld -r -T scripts/module.lds -o {module}.ko {module}.o {module}.mod.o \
                     .module-common.o"
                ),
                None,
                &[],
            ));
        }
        plan
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
        // resolve member paths at read time), recursively through nested
        // thin archives.
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
            assert!(aggregate.contains(dep), "vmlinux.o dep set missing {dep}");
        }
        assert!(rendered.contains("contentAddressed = false;"));
    }

    #[test]
    fn trims_dep_sets_to_thin_archive_consumers() {
        let rendered = render_units_nix(&module_plan(), true).expect("render");
        let unit_body = |target: &str| {
            rendered
                .split(&format!("\"{target}\" = mkUnit {{"))
                .nth(1)
                .unwrap_or_else(|| panic!("{target} unit rendered"))
                .split("};")
                .next()
                .expect("unit body")
                .to_owned()
        };

        // `ld -r` outputs and modpost dumps are self-contained, so consumers
        // stop at their direct deps: the full transitive closure re-linked
        // every .ko after any single-module body edit, because each .ko rode
        // through <mod>.mod.o -> Module.symvers -> every module object.
        let ko = unit_body("drivers/net/dummy.ko");
        for absent in [
            "units.\"Module.symvers\"",
            "units.\"vmlinux.o\"",
            "units.\"fs/nfs/nfs.o\"",
            "units.\"fs/nfs/client.o\"",
        ] {
            assert!(!ko.contains(absent), "dummy.ko must not carry {absent}");
        }

        let mod_o = unit_body("drivers/net/dummy.mod.o");
        assert!(mod_o.contains("units.\"Module.symvers\""));
        assert!(!mod_o.contains("units.\"vmlinux.o\""));

        // modpost reads the self-contained vmlinux.o, never the thin
        // archives (or their members) behind it.
        let symvers = unit_body("vmlinux.symvers");
        assert!(symvers.contains("units.\"vmlinux.o\""));
        assert!(!symvers.contains("units.\"vmlinux.a\""));
        assert!(!symvers.contains("units.\"kernel/fork.o\""));

        // The vmlinux link is exempt: link-vmlinux.sh links the
        // KBUILD_VMLINUX_OBJS/LIBS archives directly (an env-driven list no
        // .cmd records), so it keeps the full transitive closure.
        let link = unit_body("vmlinux");
        for dep in [
            "units.\"vmlinux.o\"",
            "units.\"vmlinux.a\"",
            "units.\"kernel/fork.o\"",
        ] {
            assert!(link.contains(dep), "vmlinux link closure missing {dep}");
        }
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
    fn wires_command_units_from_argv_references() {
        let mut plan = sample_plan();
        // EFI libstub (defconfig): a dep-tracked compile, an if_changed
        // stubcopy with no dep tracking, and the archive over the copies.
        plan.cmds.push(entry(
            "drivers/firmware/efi/libstub/alignedmem.o",
            "gcc -c -o drivers/firmware/efi/libstub/alignedmem.o \
             drivers/firmware/efi/libstub/alignedmem.c",
            Some("drivers/firmware/efi/libstub/alignedmem.c"),
            &[],
        ));
        plan.cmds.push(entry(
            "drivers/firmware/efi/libstub/alignedmem.stub.o",
            "strip --strip-debug -o drivers/firmware/efi/libstub/alignedmem.stub.o \
             drivers/firmware/efi/libstub/alignedmem.o; objcopy \
             --remove-section=.note.gnu.property drivers/firmware/efi/libstub/alignedmem.o \
             drivers/firmware/efi/libstub/alignedmem.stub.o",
            None,
            &[],
        ));
        plan.cmds.push(entry(
            "drivers/firmware/efi/libstub/lib.a",
            "rm -f drivers/firmware/efi/libstub/lib.a; ar cDPrsT \
             drivers/firmware/efi/libstub/lib.a drivers/firmware/efi/libstub/alignedmem.stub.o",
            None,
            &[],
        ));
        plan.cmds[4].cmd = "ld -r -o vmlinux.o --whole-archive vmlinux.a --no-whole-archive \
             --start-group drivers/firmware/efi/libstub/lib.a --end-group"
            .to_owned();

        let rendered = render_units_nix(&plan, true).expect("render");
        let stub = rendered
            .split("\"drivers/firmware/efi/libstub/alignedmem.stub.o\" = mkUnit {")
            .nth(1)
            .expect("stub.o unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        assert!(stub.contains("units.\"drivers/firmware/efi/libstub/alignedmem.o\""));
        let aggregate = rendered
            .split("\"vmlinux.o\" = mkUnit {")
            .nth(1)
            .expect("vmlinux.o unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        assert!(aggregate.contains("units.\"drivers/firmware/efi/libstub/lib.a\""));
        assert!(aggregate.contains("units.\"drivers/firmware/efi/libstub/alignedmem.stub.o\""));
    }

    #[test]
    fn wires_module_units() {
        let rendered = render_units_nix(&module_plan(), true).expect("render");

        // The modules modpost depends on vmlinux.o (argv) and on every
        // module object listed in modules.order (plan.modules), and installs
        // the .mod.c sources it regenerates for the .mod.o compiles.
        let modpost = rendered
            .split("\"Module.symvers\" = mkUnit {")
            .nth(1)
            .expect("Module.symvers unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        for dep in [
            "units.\"vmlinux.o\"",
            "units.\"drivers/net/dummy.o\"",
            "units.\"fs/nfs/nfs.o\"",
        ] {
            assert!(modpost.contains(dep), "Module.symvers missing {dep}");
        }
        for install in ["\"drivers/net/dummy.mod.c\"", "\"fs/nfs/nfs.mod.c\""] {
            assert!(
                modpost.contains(install),
                "Module.symvers must install {install}"
            );
        }

        // Each .mod.o compile consumes its modpost-provided source.
        let mod_o = rendered
            .split("\"drivers/net/dummy.mod.o\" = mkUnit {")
            .nth(1)
            .expect("dummy.mod.o unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        assert!(mod_o.contains("units.\"Module.symvers\""));
        assert!(
            !rendered.contains("srcFile \"kbuild-src-dummy.mod.c\""),
            "provided .mod.c must not become a source farm entry"
        );

        // The .ko link scans its member objects from argv.
        let ko = rendered
            .split("\"drivers/net/dummy.ko\" = mkUnit {")
            .nth(1)
            .expect("dummy.ko unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        for dep in [
            "units.\"drivers/net/dummy.o\"",
            "units.\"drivers/net/dummy.mod.o\"",
            "units.\".module-common.o\"",
        ] {
            assert!(ko.contains(dep), "dummy.ko missing {dep}");
        }

        // The ksymtab export chain: modpost provides .vmlinux.export.c, the
        // export compile consumes it, the link consumes the object.
        let symvers = rendered
            .split("\"vmlinux.symvers\" = mkUnit {")
            .nth(1)
            .expect("vmlinux.symvers unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        assert!(symvers.contains("\".vmlinux.export.c\""));
        let export_o = rendered
            .split("\".vmlinux.export.o\" = mkUnit {")
            .nth(1)
            .expect(".vmlinux.export.o unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        assert!(export_o.contains("units.\"vmlinux.symvers\""));
        let link = rendered
            .split("\"vmlinux\" = mkUnit {")
            .nth(1)
            .expect("vmlinux unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        assert!(link.contains("units.\".vmlinux.export.o\""));

        // Modules are roots (nothing module-side may be pruned) and surface
        // through the exports.
        assert!(rendered.contains("moduleSymvers = units.\"Module.symvers\";"));
        assert!(rendered.contains(
            "{ name = \"drivers/net/dummy.ko\"; path = \"${units.\"drivers/net/dummy.ko\"}/\" + \"drivers/net/dummy.ko\"; }"
        ));
        assert!(rendered.contains("{ name = \"fs/nfs/nfs.ko\";"));
        assert!(rendered.contains("{ name = \"vmlinux\"; path = units.\"vmlinux\"; }"));
        assert!(rendered.contains("{ name = \"kernel/fork.o\"; path = units.\"kernel/fork.o\"; }"));
    }

    #[test]
    fn expands_response_file_members() {
        let rendered = render_units_nix(&module_plan(), true).expect("render");
        // The multi-object module .o links `@fs/nfs/nfs.mod`; its harvested
        // contents wire the member compiles.
        let nfs = rendered
            .split("\"fs/nfs/nfs.o\" = mkUnit {")
            .nth(1)
            .expect("nfs.o unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        assert!(nfs.contains("units.\"fs/nfs/client.o\""));
    }

    #[test]
    fn rejects_uncaptured_response_file() {
        let mut plan = module_plan();
        for entry in &mut plan.cmds {
            if entry.target == "fs/nfs/nfs.o" {
                entry.at_file_contents.clear();
            }
        }
        let err = render_units_nix(&plan, true).expect_err("missing @file must fail");
        assert!(err.to_string().contains("@fs/nfs/nfs.mod"));
    }

    #[test]
    fn rejects_modules_order_entry_without_unit() {
        let mut plan = module_plan();
        plan.modules.push("drivers/net/ghost.o".to_owned());
        let err = render_units_nix(&plan, true).expect_err("unknown module must fail");
        assert!(err.to_string().contains("drivers/net/ghost.o"));
    }

    #[test]
    fn module_less_plan_renders_null_module_exports() {
        let rendered = render_units_nix(&sample_plan(), true).expect("render");
        assert!(rendered.contains("moduleSymvers = null;"));
        assert!(!rendered.contains(".ko\""), "no module unit may render");
        assert!(
            rendered.contains("pkgs.linkFarm \"kbuild-modules\" [\n\n  ];"),
            "modules farm must be empty"
        );
        // The closure aggregation still spans every reachable unit.
        assert!(rendered.contains("{ name = \"vmlinux\"; path = units.\"vmlinux\"; }"));
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
            modules: vec![],
        };
        let err = render_units_nix(&plan, false).expect_err("missing link must fail");
        assert!(err.to_string().contains("no vmlinux link command"));
    }

    #[test]
    fn scopes_compile_units_to_tracked_srctree_files() {
        let mut plan = sample_plan();
        plan.cmds[0].deps = vec![
            "include/linux/sched.h".to_owned(),
            "./include/linux/mm.h".to_owned(),
            // Toolchain header: rides in via the compiler, not the tree.
            "/nix/store/abc-gcc/include/stdarg.h".to_owned(),
            // Snapshot member: overlaid at build time.
            "include/generated/autoconf.h".to_owned(),
        ];

        let rendered = render_units_nix(&plan, true).expect("render");
        let fork = rendered
            .split("\"kernel/fork.o\" = mkUnit {")
            .nth(1)
            .expect("fork.o unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        for rel in [
            "\"kernel/fork.c\"",
            "\"include/linux/sched.h\"",
            "\"include/linux/mm.h\"",
        ] {
            assert!(fork.contains(rel), "fork.o scope missing {rel}");
        }
        assert!(!fork.contains("stdarg.h"));
        assert!(!fork.contains("include/generated/autoconf.h"));
        // Every scoped file resolves through a farm entry.
        assert!(rendered.contains(
            "\"include/linux/mm.h\" = srcFile \"kbuild-src-mm.h\" \"include/linux/mm.h\";"
        ));
        assert!(
            rendered
                .contains("\"kernel/fork.c\" = srcFile \"kbuild-src-fork.c\" \"kernel/fork.c\";")
        );
    }

    #[test]
    fn normalizes_dotdot_dep_spellings_to_one_farm_entry() {
        let mut plan = sample_plan();
        // kbuild records cpp -MD prerequisites verbatim: the same header can
        // appear relative to the object dir and relative to the srctree.
        plan.cmds[0].deps = vec![
            "arch/x86/mm/../include/asm/trace/./exceptions.h".to_owned(),
            "arch/x86/include/asm/trace/exceptions.h".to_owned(),
        ];

        let rendered = render_units_nix(&plan, true).expect("render");
        assert_eq!(
            rendered
                .matches("\"arch/x86/include/asm/trace/exceptions.h\" = srcFile")
                .count(),
            1,
            "duplicate spellings must collapse to one farm entry"
        );
        assert!(
            !rendered.contains(".."),
            "no unnormalized path may survive rendering"
        );
    }

    #[test]
    fn rejects_source_paths_escaping_the_tree() {
        let mut plan = sample_plan();
        plan.cmds[0].deps = vec!["../outside.h".to_owned()];
        let err = render_units_nix(&plan, true).expect_err("escape must fail");
        assert!(err.to_string().contains("escapes the source tree"));
    }

    #[test]
    fn scopes_the_link_to_makefiles_and_header_trees() {
        let rendered = render_units_nix(&sample_plan(), true).expect("render");
        let link = rendered
            .split("\"vmlinux\" = mkUnit {")
            .nth(1)
            .expect("vmlinux unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        for rel in [
            "\"Makefile\"",
            "\"arch/x86/Makefile.postlink\"",
            "\"init/Makefile\"",
            "\"init/version-timestamp.c\"",
        ] {
            assert!(link.contains(rel), "link scope missing {rel}");
        }
        let dirs = link
            .split("sourceDirs = [")
            .nth(1)
            .expect("link has sourceDirs")
            .split("];")
            .next()
            .expect("sourceDirs body");
        for dir in ["\"arch/x86/include\"", "\"include\"", "\"scripts\""] {
            assert!(dirs.contains(dir), "link sourceDirs missing {dir}");
        }
    }

    #[test]
    fn scopes_grep_read_member_list_files_for_archives() {
        let rendered = render_units_nix(&sample_plan(), true).expect("render");
        let archive = rendered
            .split("\"vmlinux.a\" = mkUnit {")
            .nth(1)
            .expect("vmlinux.a unit rendered")
            .split("};")
            .next()
            .expect("unit body");
        // cmd_ar_vmlinux.a reads the head-object list through `grep -F -f`;
        // the scope must carry it or the reorder silently no-ops.
        assert!(archive.contains("\"scripts/head-object-list.txt\""));
        assert!(rendered.contains(
            "\"scripts/head-object-list.txt\" = srcFile \"kbuild-src-head-object-list.txt\" \
             \"scripts/head-object-list.txt\";"
        ));
    }

    #[test]
    fn aggregation_units_carry_no_source_scope() {
        let rendered = render_units_nix(&sample_plan(), true).expect("render");
        for target in [
            "\"kernel/built-in.a\"",
            "\"vmlinux.o\"",
            "\"vmlinux.symvers\"",
        ] {
            let body = rendered
                .split(&format!("{target} = mkUnit {{"))
                .nth(1)
                .unwrap_or_else(|| panic!("{target} unit rendered"))
                .split("};")
                .next()
                .expect("unit body");
            assert!(
                !body.contains("sourceFiles"),
                "{target} must not scope sources"
            );
            assert!(!body.contains("sourceDirs"), "{target} must not scope dirs");
        }
    }

    #[test]
    fn rejects_ambiguous_srcarch() {
        let mut plan = sample_plan();
        plan.cmds.push(entry(
            "arch/arm64/kernel/setup.o",
            "gcc -c -o arch/arm64/kernel/setup.o arch/arm64/kernel/setup.c",
            Some("arch/arm64/kernel/setup.c"),
            &[],
        ));
        let err = render_units_nix(&plan, false).expect_err("two arches must fail");
        assert!(err.to_string().contains("multiple srcarch"));
    }

    #[test]
    fn nix_string_escapes_interpolation() {
        assert_eq!(nix_string(r#"a "b" ${c} \d"#), r#""a \"b\" \${c} \\d""#);
    }
}
