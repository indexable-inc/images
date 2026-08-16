//! Which builtins coerce their arguments, re-derived from cppnix's own
//! sources and compared against this crate's table.
//!
//! Four fixes in a row were one builtin each: `concatStringsSep` did not
//! coerce a set (ENG-12628), the path family and `toJSON` did not either
//! (ENG-12669, ENG-12670), and `stringLength` and `substring` rejected a path
//! cppnix copies into the store (ENG-12854). Every one was found by a sweep
//! over nixpkgs rather than by a test, and the last one arrived as an
//! ordinary type error with no refusal token, so it was invisible to the
//! refusal census as well.
//!
//! The enumerable source is cppnix. A primop either calls `coerceToString` on
//! an argument -- in which case a path, a derivation and a set carrying
//! `__toString` are all legal there -- or it calls `forceString` and a
//! non-string is a type error. This test reads which, out of `src/libexpr`,
//! and holds the crate's declarations to it.
//!
//! **What this proves and what it does not.** For a position tagged
//! `ArgType::Coerce`, the table is not a declaration: the builtin driver runs
//! the coercion and replaces the argument, so agreeing with the C++ here is
//! agreeing about behaviour. For the handful of sites the driver cannot own
//! -- a list element, an attribute value, and `dirOf`, whose primop branches
//! on the argument's type before coercing -- `BODY_COERCIONS` is a
//! declaration, and a reviewer still has to check that the body named in the
//! row passes those flags. `find_file` is why that residue is written down
//! rather than assumed away: it declared cppnix's flags in a comment and
//! called the constructor for `builtins.toString`'s.
//!
//! **The scan keys on `coerceToString`, so a coercion spelled another way is
//! invisible to it -- not listed, not excused, absent.** `builtins.toJSON` is
//! the one that matters today: `value-to-json.cc` calls
//! `tryAttrsToString(pos, v, context, false, false)` directly and has its own
//! `copyToStore` branch for the path case, so it appears in neither the table
//! nor `UNATTRIBUTED`, and nothing here would notice its flags changing.
//! ENG-12670 fixed it and `eval-okay-path-coerce` is what holds it. Extending
//! the scan to `tryAttrsToString` call sites is ENG-12906.
//!
//! Being listed and being checked are different. A site this cannot classify
//! is named in `UNATTRIBUTED` and a new one fails the build; a site it
//! classifies is compared. A site it never sees does neither, which is why
//! the sentence above says which spelling it keys on.

use nix_eval_rs::builtins::{ArgType, TABLE};
use nix_eval_rs::eval::{EvalError, eval_str};
use nix_eval_rs::print::CoerceFlags;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// -- the C++ side -----------------------------------------------------------

/// Where in a primop's arguments a coercion happens. cppnix spells the three
/// differently and they are not interchangeable: an argument position is one
/// the driver can own, the other two are inside a walk the body performs.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Site {
    /// `*args[N]`.
    Arg(usize),
    /// `*elem`, `*elems[i]`, `*v2`: an element of a list argument.
    Element,
    /// `*i->value`, `*attr.value`: an attribute of a set argument.
    AttrValue,
}

/// A coercion cppnix performs, or this crate performs, keyed the same way.
type Rows = BTreeMap<(String, Site), CoerceFlags>;

fn libexpr() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/libexpr")
}

fn cc_sources(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            cc_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "cc")
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push((name, text));
        }
    }
}

/// The arguments of the call whose `(` is at byte offset `open`, split on
/// top-level commas.
///
/// String literals are stepped over rather than scanned: cppnix's error
/// contexts contain parentheses ("the first argument (the start offset)
/// passed to builtins.substring") and commas, and counting those as
/// structure moves every later argument one place along, which silently
/// turns a flag into whatever was beside it. `None` means the parentheses do
/// not close, which is a scan that has lost its place rather than a call with
/// no arguments.
fn call_args(text: &str, open: usize) -> Option<Vec<String>> {
    let body = text.get(open..)?;
    let mut depth = 0usize;
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '"' {
            // Consume to the closing quote, honouring backslash escapes.
            let mut escaped = false;
            for (_, d) in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if d == '\\' {
                    escaped = true;
                } else if d == '"' {
                    break;
                }
            }
            cur.push_str("\"...\"");
            continue;
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    parts.push(normalise(&cur));
                    return Some(parts);
                }
            }
            ',' if depth == 1 => {
                parts.push(normalise(&cur));
                cur.clear();
                continue;
            }
            _ => {}
        }
        if !(i == 0 && c == '(') {
            cur.push(c);
        }
    }
    None
}

fn normalise(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The name of the function a byte offset sits in: the nearest line above it
/// that starts in column 0 and opens a parameter list.
///
/// Structural rather than a regex over signatures, because the signatures are
/// not regular -- `fetchTree`'s has a defaulted `const FetchTreeParams &
/// params = {}`, whose brace defeats any "parameters contain no brace"
/// pattern, and a scan that quietly skipped it would report that cppnix does
/// not coerce `fetchTree`'s first argument. Every definition in this tree is
/// clang-formatted, so column 0 is the definition and everything inside a
/// body is indented.
fn enclosing_function(text: &str, at: usize) -> Option<String> {
    let head = text.get(..at)?;
    let mut found = None;
    for (start, line) in line_starts(head) {
        let _ = start;
        // `.impl = [](...)` is a primop body too, named by the `.name` above
        // it; that case is resolved by the caller, which sees the marker.
        if let Some(rest) = line.strip_prefix("static ").or_else(|| {
            if line.starts_with(char::is_alphabetic) {
                Some(line)
            } else {
                None
            }
        }) && let Some(paren) = rest.find('(')
            && let Some(name) = rest.get(..paren).and_then(|h| h.split_whitespace().last())
        {
            // `BackedStringView EvalState::coerceToString(` is a definition
            // like any other; keeping the qualifier would make it match no
            // rule and hand the call to whichever free function was defined
            // above it, which is how four calls in eval.cc came to be
            // attributed to `copyContext` and `mkString`.
            let name = name.trim_start_matches(['*', '&']);
            let name = name.rsplit("::").next().unwrap_or(name);
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                found = Some(name.to_owned());
            }
        }
        if line.trim_start().starts_with(".impl = [](") {
            found = Some("<lambda>".to_owned());
        }
    }
    found
}

/// Whether the byte at `at` sits on a line that begins in column 0, which
/// in this clang-formatted tree means a definition rather than a statement.
fn on_unindented_line(text: &str, at: usize) -> bool {
    let start = text.get(..at).map_or(0, |h| match h.rfind('\n') {
        Some(i) => i + 1,
        None => 0,
    });
    text.get(start..at)
        .is_some_and(|indent| !indent.is_empty() && !indent.starts_with(' ') || start == at)
        || text
            .get(start..)
            .is_some_and(|line| !line.starts_with([' ', '\t']))
}

fn line_starts(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut off = 0;
    text.lines().map(move |l| {
        let here = off;
        off += l.len() + 1;
        (here, l)
    })
}

/// The `.name = "..."` nearest above `at`, which for a `RegisterPrimOp`
/// literal is the primop the `.impl` below it registers.
fn nearest_registered_name(text: &str, at: usize) -> Option<String> {
    let head = text.get(..at)?;
    let mut found = None;
    for (_, line) in line_starts(head) {
        let trimmed = line.trim().trim_start_matches('{').trim_start();
        if let Some(rest) = trimmed.strip_prefix(".name = \"")
            && let Some(name) = rest.split('"').next()
        {
            found = Some(name.to_owned());
        }
    }
    found
}

/// Functions that are not `prim_*` and that a primop delegates its coercion
/// to. Hand-written, and the `every_unattributed_coercion_is_named` test is
/// what keeps the list honest: a new helper with a coercion in it fails there
/// until it is either mapped here or excused by name.
const DELEGATES: &[(&str, &[&str])] = &[
    // `prim_derivationStrict` forwards to it whole (`primops.cc`).
    ("derivationStrictInternal", &["derivationStrict"]),
    // One body under two `FetchTreeParams` (`fetchTree.cc`), registered as
    // both names.
    ("fetchTree", &["fetchTree", "fetchGit"]),
];

/// Coercions in `src/libexpr` that belong to no primop, each with the reason
/// nothing here can attribute it. Listed rather than filtered out, because a
/// filter that drops what it cannot classify is how a new coercing primop
/// arrives unnoticed.
const UNATTRIBUTED: &[(&str, &str)] = &[
    // `ExprConcatStrings::eval`: string interpolation, whose flags this crate
    // carries as `Coerce::interpolating` and whose corpus coverage is
    // `eval-okay-path-coerce` and ENG-12447's pair. Not a primop, so there is
    // no name for the table to key it on.
    (
        "eval",
        "string interpolation in ExprConcatStrings, not a primop",
    ),
    // `EvalState::realiseString` and `EvalState::coerceToPath`: library
    // entry points several primops reach through, modelled here as
    // `Coerce::to_path` and checked by `eval-okay-path-coerce`.
    (
        "realiseString",
        "an EvalState entry point, reached by many primops",
    ),
    (
        "coerceToPath",
        "an EvalState entry point, reached by many primops",
    ),
    (
        "realisePath",
        "an EvalState entry point, reached by many primops",
    ),
    ("coerceToString", "coerceToString's own recursive calls"),
    (
        "coerceToStorePath",
        "an EvalState entry point, reached by many primops",
    ),
    (
        "tryAttrsToString",
        "the __toString arm of coerceToString itself",
    ),
];

/// Every coercion cppnix performs on a primop argument, by registered name
/// with the `__` prefix stripped, and the unattributable ones beside them.
fn cpp_coercions() -> (Rows, Vec<(String, String)>) {
    let mut sources = Vec::new();
    cc_sources(&libexpr(), &mut sources);
    assert!(
        sources.len() > 5,
        "found {} C++ sources under {}, so this test would pass by reading \
         almost nothing",
        sources.len(),
        libexpr().display()
    );
    let mut rows: Rows = BTreeMap::new();
    let mut unattributed: Vec<(String, String)> = Vec::new();
    for (file, text) in &sources {
        for (at, _) in match_positions(text, "coerceToString") {
            // The declaration and the definition in eval.cc, and the
            // ExternalValueBase member: a call has a `(` right after it.
            let Some(open) = text.get(at..).and_then(|t| t.find('(')).map(|o| at + o) else {
                continue;
            };
            if text
                .get(at + "coerceToString".len()..open)
                .is_some_and(|between| !between.trim().is_empty())
            {
                continue;
            }
            // A definition, not a call. Recognised by its indentation:
            // clang-format puts a definition's signature in column 0 and
            // indents every statement inside a body, so a `coerceToString`
            // on an unindented line is the one being defined. Reading the
            // parameter list instead is what let `ExternalValueBase`'s
            // overload -- whose `const PosIdx & pos` is spelled differently
            // from every other -- be counted as a call.
            if on_unindented_line(text, at) {
                continue;
            }
            let Some(args) = call_args(text, open) else {
                unreachable!("{file}: unbalanced parentheses at a coerceToString call")
            };
            let Some(names) = names_at(text, at) else {
                continue;
            };
            // Not a primop's coercion: the second pass below records which
            // function it was in, and `every_unattributed_coercion_is_named`
            // is where an unclassifiable one has to be accounted for. The
            // flags are not read here, because these are the calls that pass
            // variables -- `coerceToString`'s own recursion forwards
            // `coerceMore` and `copyToStore` by name.
            if names.is_empty() {
                continue;
            }
            let Some(site) = site_of(&args) else {
                // Not an argument of the primop: `coerceToString` applied to
                // a local the scan cannot trace. Attributing it to a
                // position would be a guess.
                continue;
            };
            let Some(flags) = flags_of(&args) else {
                unreachable!(
                    "{file}: cannot read the flags of a coerceToString call in {names:?}: {args:?}"
                )
            };
            for name in names {
                if let Some(prev) = rows.insert((name.clone(), site), flags) {
                    assert_eq!(
                        prev, flags,
                        "{file}: two coerceToString calls in {name} at the same kind \
                         of site disagree about the flags, so one row cannot \
                         describe both"
                    );
                }
            }
        }
        // The functions carrying a coercion that resolve to no primop.
        for (at, _) in match_positions(text, "coerceToString") {
            if !on_unindented_line(text, at)
                && names_at(text, at).is_some_and(|n| n.is_empty())
                && let Some(f) = enclosing_function(text, at)
            {
                unattributed.push((f, file.clone()));
            }
        }
    }
    (rows, unattributed)
}

fn match_positions(text: &str, needle: &str) -> Vec<(usize, ())> {
    text.match_indices(needle).map(|(i, _)| (i, ())).collect()
}

/// The primop names a call site belongs to: `Some(vec![])` when the enclosing
/// function is not a primop and not a delegate, `None` when there is no
/// enclosing function at all.
fn names_at(text: &str, at: usize) -> Option<Vec<String>> {
    let f = enclosing_function(text, at)?;
    if f == "<lambda>" {
        return Some(
            nearest_registered_name(text, at)
                .map(|n| vec![strip_prefix(&n)])
                .unwrap_or_default(),
        );
    }
    if let Some(bare) = f.strip_prefix("prim_") {
        // The registered spelling is what `RegisterPrimOp` says, and it is
        // not always the function's suffix (`prim_stringLength` registers
        // `__stringLength`). Take it from the registration when there is
        // one, and fall back to the suffix otherwise.
        let registered = registered_name_for(text, &f).unwrap_or_else(|| bare.to_owned());
        return Some(vec![strip_prefix(&registered)]);
    }
    for (helper, primops) in DELEGATES {
        if *helper == f {
            return Some(primops.iter().map(|p| strip_prefix(p)).collect());
        }
    }
    Some(Vec::new())
}

fn strip_prefix(name: &str) -> String {
    name.strip_prefix("__").unwrap_or(name).to_owned()
}

/// The `.name` of the `RegisterPrimOp` whose `.impl` is `func`.
fn registered_name_for(text: &str, func: &str) -> Option<String> {
    let needle = format!(".impl = {func}");
    let at = text.find(&needle)?;
    nearest_registered_name(text, at)
}

/// Which argument the call coerces, from the second argument's text.
fn site_of(args: &[String]) -> Option<Site> {
    let subject = args.get(1)?;
    if let Some(rest) = subject.strip_prefix("*args[")
        && let Some(idx) = rest.split(']').next()
        && let Ok(n) = idx.parse::<usize>()
    {
        return Some(Site::Arg(n));
    }
    if subject.contains("->value") || subject.contains("attr.value") {
        return Some(Site::AttrValue);
    }
    if subject.starts_with("*elem") || subject.starts_with("*v2") {
        return Some(Site::Element);
    }
    None
}

/// cppnix's `coerceMore` and `copyToStore`, positions 4 and 5 of the call,
/// each a literal or absent. `None` means the call passes something this
/// cannot read, which fails the test rather than defaulting.
fn flags_of(args: &[String]) -> Option<CoerceFlags> {
    let read = |i: usize, default: bool| -> Option<bool> {
        match args.get(i).map(String::as_str) {
            None => Some(default),
            Some("true") => Some(true),
            Some("false") => Some(false),
            Some(_) => None,
        }
    };
    // The third flag, `canonicalizePath`, is left at its default by every
    // primop; a call that passed it would change the path arm and is not
    // modelled here, so it is a refusal rather than a silent read of the
    // first five.
    if args.len() > 6 {
        return None;
    }
    Some(CoerceFlags {
        coerce_more: read(4, false)?,
        copy_to_store: read(5, true)?,
    })
}

// -- the crate side ---------------------------------------------------------

/// Coercions this crate performs in a builtin's body rather than through
/// `ArgType::Coerce`, with where the flags are passed and why the driver
/// cannot own the site.
///
/// This is the declaration-only part of the gate. Each row names the call
/// site a reviewer has to read.
const BODY_COERCIONS: &[(&str, Site, CoerceFlags, &str)] = &[
    (
        "dirOf",
        Site::Arg(0),
        CoerceFlags::NEITHER,
        "builtins.rs, bi_dir_of: cppnix answers a path for a path before it \
         coerces anything, so the body needs the value and cannot have it \
         replaced by its coercion",
    ),
    (
        "concatStringsSep",
        Site::Element,
        CoerceFlags::DEFAULTS,
        "print.rs, Coerce::joining: a list element, not an argument position",
    ),
    (
        "derivationStrict",
        Site::Element,
        CoerceFlags::DERIVATION_ATTR,
        "drvstrict.rs, Task::coerce_copying: an element of the `args` \
         attribute, not an argument position",
    ),
    (
        "derivationStrict",
        Site::AttrValue,
        CoerceFlags::DERIVATION_ATTR,
        "drvstrict.rs, Task::coerce_copying: a derivation attribute, not an \
         argument position",
    ),
    (
        "findFile",
        Site::AttrValue,
        CoerceFlags::NEITHER,
        "primops_host.rs, FindStage::PathValue: the `path` attribute of a \
         search-path entry, not an argument position",
    ),
];

/// cppnix coerces these and this crate does not, each with the reason and
/// what makes it safe today. A row here is a promise that the difference is
/// not observable, not permission to differ.
const NOT_COERCED: &[(&str, Site, &str)] = &[
    (
        "addErrorContext",
        Site::Arg(0),
        "cppnix coerces the message only when the value it wraps throws \
         (`primops.cc`, prim_addErrorContext's catch block), and this \
         evaluator carries no traces yet (ENG-12714), so the message is never \
         forced at all. `addErrorContext (throw \"x\") 1` is 1 in both arms.",
    ),
    (
        "fetchTree",
        Site::Arg(0),
        "the bare-string spelling of the argument is refused by name here; \
         only the attribute-set spelling is served, and the coercion cppnix \
         performs is on the string spelling.",
    ),
    (
        "fetchGit",
        Site::Arg(0),
        "same as fetchTree: one cppnix body under two names.",
    ),
    (
        "fetchTree",
        Site::AttrValue,
        "the attribute walk classifies and forwards the attributes rather \
         than coercing them; a path attribute is handled by the fetcher, and \
         the refusal is by name where it is not.",
    ),
    (
        "fetchGit",
        Site::AttrValue,
        "same as fetchTree: one cppnix body under two names.",
    ),
];

fn implemented() -> Vec<&'static str> {
    TABLE.iter().map(|b| b.name).collect()
}

/// Every coercion this crate declares, from the table and the body list.
fn crate_coercions() -> Rows {
    let mut rows: Rows = BTreeMap::new();
    for b in TABLE {
        for &(pos, ty) in b.strict {
            if let ArgType::Coerce(flags) = ty {
                rows.insert((b.name.to_owned(), Site::Arg(pos)), flags);
            }
        }
    }
    for (name, site, flags, _) in BODY_COERCIONS {
        assert!(
            rows.insert(((*name).to_owned(), *site), *flags).is_none(),
            "{name} declares the same site twice, once in the table and once \
             in BODY_COERCIONS; one of them is not doing anything"
        );
    }
    rows
}

// -- the gate ---------------------------------------------------------------

/// The crate coerces exactly where cppnix coerces, with cppnix's flags.
#[test]
fn the_coercion_table_matches_the_cpp_sources() {
    let (cpp, _) = cpp_coercions();
    assert!(
        cpp.len() >= 12,
        "derived only {} coercion sites from the C++ sources, fewer than the \
         fork is known to carry; the scan is broken, not the table: {cpp:?}",
        cpp.len()
    );
    // Named sites, so a scan that silently stopped attributing anything
    // cannot pass by deriving twelve of something else.
    for expected in [
        ("stringLength".to_owned(), Site::Arg(0)),
        ("substring".to_owned(), Site::Arg(2)),
        ("toString".to_owned(), Site::Arg(0)),
        ("concatStringsSep".to_owned(), Site::Element),
    ] {
        assert!(
            cpp.contains_key(&expected),
            "the scan did not find cppnix's coercion of {expected:?}, which \
             is in primops.cc; every comparison below is therefore vacuous"
        );
    }

    let ours = crate_coercions();
    let implemented = implemented();
    let mut missing = Vec::new();
    for ((name, site), flags) in &cpp {
        if !implemented.contains(&name.as_str()) {
            continue;
        }
        if NOT_COERCED.iter().any(|(n, s, _)| n == name && s == site) {
            continue;
        }
        match ours.get(&(name.clone(), *site)) {
            Some(mine) if mine == flags => {}
            other => missing.push(format!(
                "{name} {site:?}: cppnix coerces with {flags:?}, this crate says {other:?}"
            )),
        }
    }
    assert!(
        missing.is_empty(),
        "cppnix coerces where this crate does not, or with other flags. Each \
         line is a value divergence in the served direction -- cppnix answers \
         and this evaluator raises a type error, which is ENG-12854 and the \
         three before it:\n  {}",
        missing.join("\n  ")
    );

    let mut invented = Vec::new();
    for ((name, site), flags) in &ours {
        match cpp.get(&(name.clone(), *site)) {
            Some(theirs) if theirs == flags => {}
            other => invented.push(format!(
                "{name} {site:?}: this crate coerces with {flags:?}, cppnix says {other:?}"
            )),
        }
    }
    assert!(
        invented.is_empty(),
        "this crate coerces where cppnix does not, or with other flags. \
         Accepting a program cppnix rejects is the same divergence in the \
         other direction, and a wrong `copyToStore` is a wrong store path in \
         the answer:\n  {}",
        invented.join("\n  ")
    );
}

/// Every coercion in `src/libexpr` is either attributed to a primop or
/// excused by name. The list of excuses is the part a human maintains, so it
/// is short and each entry says why nothing here can classify it.
#[test]
fn every_unattributed_coercion_is_named() {
    let (_, unattributed) = cpp_coercions();
    assert!(
        unattributed.len() >= 5,
        "found {} coercions outside a primop, which is fewer than eval.cc \
         alone carries; the scan is attributing everything to something and \
         this test is vacuous",
        unattributed.len()
    );
    let mut surprises: Vec<String> = unattributed
        .iter()
        .filter(|(f, _)| !UNATTRIBUTED.iter().any(|(name, _)| name == f))
        .map(|(f, file)| format!("{f} ({file})"))
        .collect();
    surprises.sort_unstable();
    surprises.dedup();
    assert!(
        surprises.is_empty(),
        "these functions coerce and this test cannot say which primop they \
         belong to: {surprises:?}. Map the function in DELEGATES, or add it \
         to UNATTRIBUTED with the reason. Leaving it out means a coercing \
         primop that the table does not have to declare."
    );
}

/// A position cppnix takes with `forceString` or `forceStringNoCtx` is not a
/// coercion site, and tagging it `ArgType::Coerce` would accept a path
/// cppnix rejects.
#[test]
fn a_forced_string_position_is_never_tagged_coerce() {
    let mut sources = Vec::new();
    cc_sources(&libexpr(), &mut sources);
    let mut forced: Vec<(String, usize)> = Vec::new();
    for (_, text) in &sources {
        for needle in ["forceString(", "forceStringNoCtx("] {
            for (at, _) in match_positions(text, needle) {
                let open = at + needle.len() - 1;
                let Some(args) = call_args(text, open) else {
                    continue;
                };
                let Some(names) = names_at(text, at) else {
                    continue;
                };
                let Some(first) = args.first() else { continue };
                let Some(rest) = first.strip_prefix("*args[") else {
                    continue;
                };
                let Some(Ok(n)) = rest.split(']').next().map(str::parse::<usize>) else {
                    continue;
                };
                for name in names {
                    forced.push((name, n));
                }
            }
        }
    }
    assert!(
        forced.len() >= 15,
        "found only {} forceString positions, so this test is checking almost \
         nothing",
        forced.len()
    );
    let ours = crate_coercions();
    let mut wrong = Vec::new();
    for (name, pos) in &forced {
        if ours.contains_key(&(name.clone(), Site::Arg(*pos))) {
            wrong.push(format!("{name} argument {pos}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "cppnix takes these positions with forceString and this crate coerces \
         them, so it accepts paths and sets cppnix rejects: {wrong:?}"
    );
}

// -- the behavioural half ---------------------------------------------------

/// A store that answers with a path derived from the source path, so a test
/// can tell a coerced path from an uncoerced one without a real store.
fn fake_copy(path: &str) -> Result<String, String> {
    let base = path.rsplit('/').next().unwrap_or("x");
    Ok(format!("/nix/store/{}-{base}", "0".repeat(32)))
}

/// One expression per coercing builtin, with `SUBJECT` where the coerced
/// argument goes. Every builtin the gate says coerces an argument position
/// must appear here, which is what stops a row being added to the table
/// without anything ever running it.
const PROBES: &[(&str, &str)] = &[
    ("stringLength", "builtins.stringLength SUBJECT"),
    ("substring", "builtins.substring 0 11 SUBJECT"),
    ("toString", "builtins.toString SUBJECT"),
    ("baseNameOf", "builtins.baseNameOf SUBJECT"),
    ("dirOf", "builtins.dirOf SUBJECT"),
    (
        "throw",
        "(builtins.tryEval (builtins.throw SUBJECT)).success",
    ),
    ("abort", "builtins.typeOf SUBJECT"),
    (
        "unsafeDiscardStringContext",
        "builtins.unsafeDiscardStringContext SUBJECT",
    ),
    (
        "unsafeDiscardOutputDependency",
        "builtins.unsafeDiscardOutputDependency SUBJECT",
    ),
    (
        "addDrvOutputDependencies",
        "builtins.stringLength (builtins.unsafeDiscardStringContext SUBJECT)",
    ),
];

/// Every builtin whose argument position the gate says is coerced has a probe
/// that runs it. Without this, a row can be added to the table and be wrong
/// in the body, and the class gate above would still pass.
#[test]
fn every_coercing_argument_position_has_a_probe() {
    let mut unprobed: Vec<String> = crate_coercions()
        .keys()
        .filter(|(_, site)| matches!(site, Site::Arg(_)))
        .map(|(name, _)| name.clone())
        .filter(|name| !PROBES.iter().any(|(n, _)| n == name))
        .collect();
    unprobed.sort_unstable();
    unprobed.dedup();
    assert!(
        unprobed.is_empty(),
        "these coerce an argument and nothing evaluates them with a \
         non-string there: {unprobed:?}"
    );
    // And the probes are for builtins that exist.
    let implemented = implemented();
    for (name, _) in PROBES {
        assert!(
            implemented.contains(name),
            "{name} has a probe and is not implemented"
        );
    }
}

/// A set coerces at every position the table says coerces: through
/// `__toString`, and through `outPath` for a derivation.
///
/// This is the half the C++ scan cannot check. A body that reached for
/// `want_str` would raise "expected a string but found a set" here.
#[test]
fn a_set_coerces_at_every_coercing_position() {
    for (name, probe) in PROBES {
        for subject in [
            r#"{ __toString = self: "/nix/store/00000000000000000000000000000000-x"; }"#,
            r#"{ type = "derivation"; outPath = "/nix/store/00000000000000000000000000000000-x"; }"#,
        ] {
            let src = probe.replace("SUBJECT", subject);
            let out = eval_str(&src);
            assert!(
                out.is_ok(),
                "{name} rejects a set at its coerced position: {src} gave {out:?}"
            );
        }
    }
}

/// A path coerces too, and where cppnix's `copyToStore` is on the answer is
/// about the store path rather than the source path. `stringLength ./f` is
/// the length of the store path, which is ENG-12854's repro.
#[test]
fn a_path_reaches_the_store_where_copy_to_store_is_on() {
    /// Evaluate against a host whose one answer is the faked store copy.
    ///
    /// The host is this call's argument, so nothing else in the binary can
    /// see it and nothing else has to be held still while it exists.
    fn with_store(src: &str) -> Result<String, EvalError> {
        let host = nix_eval_rs::host::FnHost {
            store_copy: Some(fake_copy),
            ..nix_eval_rs::host::FnHost::default()
        };
        let mut vm = nix_eval_rs::vm::Vm::from_process_settings();
        nix_eval_rs::eval::eval_str_on(
            src,
            ".",
            nix_eval_rs::compile::Origin::String,
            &mut vm,
            &host,
        )
    }

    // 11 for "/nix/store/", 32 for the hash, 1 for the dash, and the name.
    let store_len = 11 + 32 + 1 + "lib.nix".len();
    assert_eq!(
        with_store("builtins.stringLength ./lib.nix").ok(),
        Some(store_len.to_string()),
        "stringLength of a path must be the length of the store path cppnix \
         copies it to (ENG-12854), not of the source path"
    );
    assert_eq!(
        with_store("builtins.substring 0 11 ./lib.nix").ok(),
        Some("\"/nix/store/\"".to_owned())
    );
    // `copyToStore` off: the source path, and no store question at all.
    assert_eq!(
        with_store("builtins.baseNameOf ./dir/lib.nix").ok(),
        Some("\"lib.nix\"".to_owned())
    );
}

/// `coerceMore` is off everywhere except `toString`, and that half matters as
/// much: a coercion that took every value would answer where cppnix raises a
/// type error, which is the same class of divergence pointing the other way.
#[test]
fn coerce_more_stays_off_where_cppnix_leaves_it_off() {
    for src in [
        "builtins.stringLength 42",
        "builtins.substring 0 1 42",
        "builtins.baseNameOf 42",
        "builtins.stringLength [ \"a\" ]",
    ] {
        let out = eval_str(src);
        assert!(
            matches!(&out, Err(EvalError::Eval(_, m, _)) if m.starts_with("cannot coerce")),
            "{src} must be cppnix's `cannot coerce ...` type error, got {out:?}"
        );
    }
    // toString is the one primop that sets it.
    assert_eq!(
        eval_str("builtins.toString 42").ok(),
        Some("\"42\"".to_owned())
    );
    assert_eq!(
        eval_str("builtins.toString [ 1 2 ]").ok(),
        Some("\"1 2\"".to_owned())
    );
}
