//! The mirror changelog, derived mechanically from the monorepo commits that
//! touched the package path (`git log -- packages/<pkg>`), never
//! hand-maintained. Section names follow Keep a Changelog; entries are
//! grouped by month because a mirror tracks the monorepo continuously and
//! has no versioned releases to head the sections with. A
//! conventional-commit prefix picks the section (feat -> Added, fix ->
//! Fixed, ...); any other subject lands under Changed, unstripped.

use std::fmt::Write as _;

use crate::workspace::Change;

pub struct Request<'a> {
    /// Monorepo `owner/name`, e.g. `indexable-inc/index`.
    pub monorepo: &'a str,
    /// Repo-relative package path, e.g. `packages/progress-style`.
    pub package_path: &'a str,
    pub crate_name: &'a str,
    /// Monorepo commits that touched `package_path`, newest first.
    pub history: &'a [Change],
}

/// Keep a Changelog's section names, in its canonical order.
const SECTIONS: [&str; 6] = [
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
];

pub fn compose(request: &Request<'_>) -> String {
    let Request {
        monorepo,
        package_path,
        crate_name,
        history,
    } = *request;
    let mut out = format!(
        "# Changelog\n\n\
         All notable changes to `{crate_name}`, derived mechanically from the \
         [monorepo commits](https://github.com/{monorepo}/commits/main/{package_path}) that \
         touched [`{package_path}`](https://github.com/{monorepo}/tree/main/{package_path}). \
         Section names follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the \
         mirror tracks the monorepo continuously, so entries are grouped by month instead of \
         by release.\n"
    );
    for group in by_month(history) {
        let _ = write!(out, "\n## {}\n", group.month);
        for section in SECTIONS {
            let entries: Vec<&&Change> = group
                .changes
                .iter()
                .filter(|change| classify(&change.subject).section == section)
                .collect();
            if entries.is_empty() {
                continue;
            }
            let _ = write!(out, "\n### {section}\n\n");
            for change in entries {
                out.push_str(&entry(monorepo, change));
            }
        }
    }
    out
}

/// One month of history, newest month first.
struct MonthGroup<'a> {
    /// `YYYY-MM`.
    month: &'a str,
    changes: Vec<&'a Change>,
}

/// Group by `YYYY-MM`, preserving the newest-first order of first
/// appearance. Grouping (not splitting on month changes) keeps a month whole
/// even when commit dates are not strictly monotonic.
fn by_month(history: &[Change]) -> Vec<MonthGroup<'_>> {
    let mut months: Vec<MonthGroup<'_>> = Vec::new();
    for change in history {
        let month = change.date.get(..7).unwrap_or(&change.date);
        match months.iter_mut().find(|group| group.month == month) {
            Some(group) => group.changes.push(change),
            None => months.push(MonthGroup {
                month,
                changes: vec![change],
            }),
        }
    }
    months
}

fn entry(monorepo: &str, change: &Change) -> String {
    let Classified { scope, rest, .. } = classify(&change.subject);
    let short = change.sha.get(..7).unwrap_or(&change.sha);
    let mut text = String::new();
    if let Some(scope) = scope {
        let _ = write!(text, "{}: ", subject_markdown(monorepo, scope));
    }
    text.push_str(&subject_markdown(monorepo, rest));
    format!(
        "- {text} ([`{short}`](https://github.com/{monorepo}/commit/{}), {})\n",
        change.sha, change.date
    )
}

/// A subject sorted into its Keep a Changelog section.
struct Classified<'a> {
    section: &'static str,
    /// Conventional-commit scope, kept as a prefix on the rendered entry.
    scope: Option<&'a str>,
    /// The subject with a recognized conventional-commit type stripped.
    rest: &'a str,
}

/// Map a subject to its Keep a Changelog section. Only a recognized
/// conventional-commit type is stripped (its scope survives as a prefix);
/// anything else keeps the whole subject, under Changed.
fn classify(subject: &str) -> Classified<'_> {
    let unclassified = Classified {
        section: "Changed",
        scope: None,
        rest: subject,
    };
    let Some((prefix, rest)) = subject.split_once(':') else {
        return unclassified;
    };
    let prefix = prefix.trim_end_matches('!');
    let (kind, scope) = match prefix.split_once('(') {
        Some((kind, scope)) => match scope.strip_suffix(')') {
            Some(scope) => (kind, Some(scope)),
            None => return unclassified,
        },
        None => (prefix, None),
    };
    let Some(section) = section_for(kind) else {
        return unclassified;
    };
    Classified {
        section,
        scope,
        rest: rest.trim_start(),
    }
}

fn section_for(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "feat" => "Added",
        "fix" => "Fixed",
        "revert" => "Removed",
        "build" | "chore" | "ci" | "docs" | "perf" | "refactor" | "style" | "test" => "Changed",
        _ => return None,
    })
}

/// A commit subject as safe inline markdown: formatting characters escaped
/// so a subject cannot inject markup, and `#123` references linked to the
/// monorepo (GitHub would otherwise autolink them to the mirror repo's own,
/// meaningless, issue numbers).
fn subject_markdown(monorepo: &str, subject: &str) -> String {
    let mut out = String::with_capacity(subject.len());
    let mut chars = subject.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' => {
                out.push('\\');
                out.push(ch);
            }
            '#' if chars.peek().is_some_and(char::is_ascii_digit) => {
                let mut number = String::new();
                while let Some(digit) = chars.next_if(char::is_ascii_digit) {
                    number.push(digit);
                }
                let _ = write!(
                    out,
                    "[#{number}](https://github.com/{monorepo}/issues/{number})"
                );
            }
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(sha: &str, date: &str, subject: &str) -> Change {
        Change {
            sha: sha.to_owned(),
            date: date.to_owned(),
            subject: subject.to_owned(),
        }
    }

    fn render(history: &[Change]) -> String {
        compose(&Request {
            monorepo: "indexable-inc/index",
            package_path: "packages/sqlmerge",
            crate_name: "sqlmerge",
            history,
        })
    }

    #[test]
    fn groups_by_month_then_keep_a_changelog_section() {
        let history = [
            change("a".repeat(40).as_str(), "2026-07-03", "feat(sqlmerge): add policies"),
            change("b".repeat(40).as_str(), "2026-07-01", "fix: reject shallow merge"),
            change("c".repeat(40).as_str(), "2026-06-20", "sqlmerge: initial import"),
        ];
        let out = render(&history);
        let july = out.find("## 2026-07").expect("july section");
        let june = out.find("## 2026-06").expect("june section");
        assert!(july < june, "newest month first:\n{out}");
        assert!(out.contains("### Added\n\n- sqlmerge: add policies"), "{out}");
        assert!(out.contains("### Fixed\n\n- reject shallow merge"), "{out}");
        // Not a conventional type: whole subject survives, under Changed.
        assert!(out.contains("### Changed\n\n- sqlmerge: initial import"), "{out}");
    }

    #[test]
    fn links_shas_and_monorepo_pull_references() {
        let history = [change(
            "0123456789abcdef0123456789abcdef01234567",
            "2026-07-03",
            "feat: land mirrors (#2022)",
        )];
        let out = render(&history);
        assert!(
            out.contains(
                "[`0123456`](https://github.com/indexable-inc/index/commit/0123456789abcdef0123456789abcdef01234567)"
            ),
            "{out}"
        );
        assert!(
            out.contains("[#2022](https://github.com/indexable-inc/index/issues/2022)"),
            "{out}"
        );
    }

    #[test]
    fn escapes_markdown_in_subjects() {
        let history = [change("d".repeat(40).as_str(), "2026-07-03", "add [bracket] <tag>")];
        let out = render(&history);
        assert!(out.contains(r"add \[bracket\] \<tag\>"), "{out}");
    }

    #[test]
    fn breaking_marker_and_malformed_scope_do_not_panic() {
        let history = [
            change("e".repeat(40).as_str(), "2026-07-03", "feat!: new wire format"),
            change("f".repeat(40).as_str(), "2026-07-02", "fix(unclosed: scope"),
        ];
        let out = render(&history);
        assert!(out.contains("### Added\n\n- new wire format"), "{out}");
        assert!(out.contains(r"- fix(unclosed: scope"), "{out}");
    }
}
