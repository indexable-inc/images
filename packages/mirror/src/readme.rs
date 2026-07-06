//! The mirror README, composed to the house README style
//! (packages/agent/skills/creating-a-readme/SKILL.md, the single source of
//! truth the skill says generators conform to). A package with its own
//! README leads the mirror with it verbatim -- the skill already makes it
//! open with an `assets/hero.svg` and a hook question, and the skill's
//! mirror checklist requires it to make sense standalone -- behind a banner
//! declaring the repo a read-only generated mirror; a derived Install
//! section is appended only when the body has none. A package without a
//! README gets the whole skill shape synthesized from metadata: hero, hook,
//! pitch (the declarative `mirror.description`, falling back to the crate's
//! `[package] description`), Install branched on what the package *is*
//! (flake-exposed? binary? library?), and a minimal Use section. Nothing is
//! hand-maintained per mirror.

use std::fmt::Write as _;

/// Where a hero lives relative to its README (the skill's convention), both
/// for a package-committed hero riding into the mirror and for the one this
/// module synthesizes.
pub const HERO_PATH: &str = "assets/hero.svg";

pub struct Package<'a> {
    /// Monorepo `owner/name`, e.g. `indexable-inc/index`.
    pub monorepo: &'a str,
    /// Repo-relative package path, e.g. `packages/progress-style`.
    pub path: &'a str,
    /// Monorepo commit the tree was generated from (full sha).
    pub commit: &'a str,
    pub crate_name: &'a str,
    /// The pitch: `mirror.description` when the caller resolved the mirror
    /// manifest, else the crate's `[package] description`.
    pub description: Option<&'a str>,
    /// The mirror repo's own `owner/name`, when known.
    pub mirror_repo: Option<&'a str>,
    /// The monorepo flake output attr (`nix run .#<attr>`) when the package
    /// is exposed on the flake.
    pub flake_attr: Option<&'a str>,
    /// The crate builds an executable (`src/main.rs`, `src/bin/`, `[[bin]]`).
    pub has_binary: bool,
    /// A generated `CHANGELOG.md` sits next to the README.
    pub has_changelog: bool,
}

pub fn compose(pkg: &Package<'_>, existing: Option<&str>) -> String {
    // A curated README already opens with its own hero and hook (the
    // creating-a-readme skill), so behind the banner the generator adds only
    // what the package cannot know about itself.
    let mut sections = existing.map_or_else(
        || {
            vec![
                hero_reference(pkg),
                banner(pkg),
                lead(pkg),
                install(pkg),
                usage(pkg),
            ]
        },
        |body| {
            let mut sections = vec![banner(pkg), body.to_owned()];
            if !has_install(body) {
                sections.push(install(pkg));
            }
            sections
        },
    );
    if pkg.has_changelog {
        sections.push(format!(
            "Changes: [CHANGELOG.md](CHANGELOG.md), derived from the \
             [monorepo history](https://github.com/{}/commits/main/{}) of the package.",
            pkg.monorepo, pkg.path
        ));
    }
    let sections: Vec<String> = sections
        .iter()
        .map(|section| format!("{}\n", section.trim_end()))
        .collect();
    sections.join("\n")
}

/// Whether the body already tells the reader how to get the package (a
/// skill-conformant README derives its own install lines); an older body
/// without any gets the generated section appended.
fn has_install(body: &str) -> bool {
    body.contains("cargo install")
        || body.contains("nix run github:")
        || body.contains("{ git = ")
}

fn hero_reference(pkg: &Package<'_>) -> String {
    let alt = pkg.description.unwrap_or(pkg.crate_name);
    format!(
        "<p align=\"center\"><img src=\"{HERO_PATH}\" width=\"720\" alt=\"{}\"></p>",
        xml_escape(alt)
    )
}

fn banner(pkg: &Package<'_>) -> String {
    let Package {
        monorepo,
        path,
        commit,
        mirror_repo,
        ..
    } = *pkg;
    let short = commit.get(..12).unwrap_or(commit);
    let subject = mirror_repo.map_or_else(
        || "This repository".to_owned(),
        |repo| format!("[`{repo}`](https://github.com/{repo})"),
    );
    format!(
        "> [!NOTE]\n\
         > {subject} is a read-only mirror, generated from \
         [`{path}`](https://github.com/{monorepo}/tree/{commit}/{path}) in \
         [`{monorepo}`](https://github.com/{monorepo}) at commit `{short}`. \
         The monorepo is the source of truth: please open issues and pull requests \
         [there](https://github.com/{monorepo}). This mirror is regenerated automatically; \
         anything pushed directly here will be overwritten."
    )
}

fn lead(pkg: &Package<'_>) -> String {
    let mut out = format!("# {}\n", pkg.crate_name);
    if let Some(description) = pkg.description {
        let _ = write!(out, "\n**{}**\n\n{description}\n", hook(description, pkg));
    }
    out
}

/// The opening hook: the description's leading clause recast as a question
/// about how little it takes to adopt the package. Mechanical on purpose:
/// the curation lives in the declarative `mirror.description` (one source of
/// truth), and the clause split leans on the house description shape
/// ("What it is: how/why.").
fn hook(description: &str, pkg: &Package<'_>) -> String {
    let clause = description
        .split_once(": ")
        .map_or(description, |(clause, _)| clause);
    let clause = clause.split_once(". ").map_or(clause, |(clause, _)| clause);
    // A trailing purpose clause ("..., so every CLI matches") is the part a
    // hook drops; a clause that is still a long sentence keeps its first limb.
    let clause = clause.split_once(", so ").map_or(clause, |(clause, _)| clause);
    let clause = if clause.len() > 72 {
        clause.split_once(", ").map_or(clause, |(clause, _)| clause)
    } else {
        clause
    };
    let clause = clause.trim_end_matches('.');
    let tail = if pkg.flake_attr.is_some() && pkg.has_binary {
        "one `nix run` away?"
    } else if pkg.has_binary {
        "one `cargo install` away?"
    } else {
        "one dependency line away?"
    };
    format!("{clause}: {tail}")
}

fn install(pkg: &Package<'_>) -> String {
    let mut out = String::from("## Install\n");
    let git_url = pkg.mirror_repo.map_or_else(
        || format!("https://github.com/{}", pkg.monorepo),
        |repo| format!("https://github.com/{repo}"),
    );
    if let Some(attr) = pkg.flake_attr {
        let verb = if pkg.has_binary { "Run" } else { "Build" };
        let command = if pkg.has_binary { "run" } else { "build" };
        let _ = write!(
            out,
            "\n{verb} it straight from the monorepo flake, nothing to clone \
             ([install Nix](https://nixos.org/download/) first):\n\n\
             ```console\nnix {command} github:{}#{attr}\n```\n",
            pkg.monorepo
        );
    }
    if pkg.has_binary {
        let lead = if pkg.flake_attr.is_some() {
            "Or install the binary with cargo:"
        } else {
            "Install the binary with cargo:"
        };
        let _ = write!(
            out,
            "\n{lead}\n\n```console\ncargo install --git {git_url} {}\n```\n",
            pkg.crate_name
        );
    } else {
        let _ = write!(
            out,
            "\n`{name}` is not on crates.io; add it as a git dependency:\n\n\
             ```toml\n[dependencies]\n{name} = {{ git = \"{git_url}\" }}\n```\n",
            name = pkg.crate_name
        );
    }
    out
}

/// The minimal usage section for a package without its own README.
fn usage(pkg: &Package<'_>) -> String {
    if pkg.has_binary {
        format!("## Use\n\n```console\n{} --help\n```\n", pkg.crate_name)
    } else {
        format!(
            "## Use\n\nAdd the dependency, then browse the API locally:\n\n\
             ```console\ncargo doc --open -p {}\n```\n",
            pkg.crate_name
        )
    }
}

/// The synthesized hero for a package that ships neither a README nor its
/// own `assets/hero.svg`: crate name and tagline with a deterministic
/// geometric mark derived from the crate name (same name, same mark; no
/// per-package art to maintain). One SVG adapts to dark/light via its
/// embedded `prefers-color-scheme` CSS, per the creating-a-readme skill.
pub fn hero_svg(name: &str, tagline: Option<&str>) -> String {
    let hash = fnv1a(name.as_bytes());
    let hue = hash % 360;
    let mut marks = String::new();
    for cell in mark_cells(hash) {
        let x = 44 + cell.col * 34;
        let y = 58 + cell.row * 34;
        let _ = writeln!(
            marks,
            "  <rect class=\"accent\" x=\"{x}\" y=\"{y}\" width=\"26\" height=\"26\" rx=\"7\"/>"
        );
    }
    // A long name shrinks instead of overflowing the fixed viewBox.
    let name_size = if name.len() > 16 { 40 } else { 52 };
    let mut tags = String::new();
    for (index, line) in tagline
        .map(|tagline| wrap(tagline, 58, 2))
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let y = 138 + 30 * index;
        let _ = writeln!(
            tags,
            "  <text class=\"muted\" font-size=\"22\" x=\"160\" y=\"{y}\">{}</text>",
            xml_escape(line)
        );
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 880 220\" role=\"img\" aria-label=\"{name}\"\n\
         \x20    font-family=\"system-ui, -apple-system, 'Segoe UI', sans-serif\">\n\
         <title>{name}</title>\n\
         <style>\n\
         svg {{ color: #1f2328; }}\n\
         text {{ fill: currentColor; }}\n\
         .muted {{ fill: #656d76; }}\n\
         .accent {{ fill: hsl({hue} 70% 45%); }}\n\
         @media (prefers-color-scheme: dark) {{\n\
         svg {{ color: #e6edf3; }}\n\
         .muted {{ fill: #8b949e; }}\n\
         .accent {{ fill: hsl({hue} 75% 65%); }}\n\
         }}\n\
         </style>\n\
         {marks}  <text font-size=\"{name_size}\" font-weight=\"700\" x=\"160\" y=\"104\">{escaped}</text>\n\
         {tags}</svg>\n",
        name = xml_escape(name),
        escaped = xml_escape(name),
    )
}

/// One filled cell of the hero mark's 3x3 grid.
struct Cell {
    col: u64,
    row: u64,
}

/// A 3x3 grid mirrored around its vertical axis (symmetry reads as a mark,
/// raw bits read as noise): bits 0-2 fill the outer columns per row, bits
/// 3-5 the middle column; a hash with none set gets the center cell.
fn mark_cells(hash: u64) -> Vec<Cell> {
    let mut cells = Vec::new();
    for row in 0..3 {
        if hash >> row & 1 == 1 {
            cells.push(Cell { col: 0, row });
            cells.push(Cell { col: 2, row });
        }
        if hash >> (3 + row) & 1 == 1 {
            cells.push(Cell { col: 1, row });
        }
    }
    if cells.is_empty() {
        cells.push(Cell { col: 1, row: 1 });
    }
    cells
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Greedy word wrap into at most `max_lines` lines of about `width` columns;
/// a truncated tagline ends in an ellipsis rather than overflowing the hero.
fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.len() + 1 + word.len() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            last.push('…');
        }
    }
    lines
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package() -> Package<'static> {
        Package {
            monorepo: "indexable-inc/index",
            path: "packages/progress-style",
            commit: "0123456789abcdef0123456789abcdef01234567",
            crate_name: "progress-style",
            description: Some("Shared indicatif styling for ix tools, so every CLI matches."),
            mirror_repo: Some("indexable-inc/progress-style"),
            flake_attr: None,
            has_binary: false,
            has_changelog: true,
        }
    }

    #[test]
    fn synthesized_readme_leads_hero_then_banner() {
        let out = compose(&package(), None);
        assert!(
            out.starts_with("<p align=\"center\"><img src=\"assets/hero.svg\" width=\"720\""),
            "{out}"
        );
        assert!(
            out.contains(
                "https://github.com/indexable-inc/index/tree/0123456789abcdef0123456789abcdef01234567/packages/progress-style"
            ),
            "{out}"
        );
        assert!(out.contains("`0123456789ab`"), "{out}");
    }

    #[test]
    fn library_gets_hook_pitch_and_git_dependency_snippet() {
        let out = compose(&package(), None);
        assert!(
            out.contains("**Shared indicatif styling for ix tools: one dependency line away?**"),
            "{out}"
        );
        assert!(
            out.contains(
                "progress-style = { git = \"https://github.com/indexable-inc/progress-style\" }"
            ),
            "{out}"
        );
        assert!(!out.contains("nix run"), "{out}");
    }

    #[test]
    fn flake_exposed_binary_gets_nix_run_and_cargo_install() {
        let pkg = Package {
            crate_name: "sqlmerge",
            mirror_repo: Some("indexable-inc/sqlmerge"),
            flake_attr: Some("sqlmerge"),
            has_binary: true,
            description: Some("A git merge driver for SQLite database files: a real merge."),
            ..package()
        };
        let out = compose(&pkg, None);
        assert!(
            out.contains("**A git merge driver for SQLite database files: one `nix run` away?**"),
            "{out}"
        );
        assert!(out.contains("nix run github:indexable-inc/index#sqlmerge"), "{out}");
        assert!(
            out.contains("cargo install --git https://github.com/indexable-inc/sqlmerge sqlmerge"),
            "{out}"
        );
    }

    #[test]
    fn binary_without_flake_attr_installs_via_cargo_only() {
        let pkg = Package {
            flake_attr: None,
            has_binary: true,
            ..package()
        };
        let out = compose(&pkg, None);
        assert!(!out.contains("nix run"), "{out}");
        assert!(out.contains("cargo install --git"), "{out}");
    }

    #[test]
    fn curated_body_rides_verbatim_behind_the_banner() {
        let body = "<p align=\"center\"><img src=\"assets/hero.svg\"></p>\n\n# sqlmerge\n\n\
                    Ever needed this? A pitch.\n\n## Get it\n\n```sh\ncargo install --git x\n```\n";
        let out = compose(&package(), Some(body));
        assert!(out.starts_with("> [!NOTE]\n"), "banner first:\n{out}");
        assert!(out.contains(body.trim_end()), "body verbatim:\n{out}");
        // The body derives its own install lines; no generated section.
        assert!(!out.contains("## Install"), "{out}");
        assert!(out.ends_with(
            "Changes: [CHANGELOG.md](CHANGELOG.md), derived from the [monorepo history](https://github.com/indexable-inc/index/commits/main/packages/progress-style) of the package.\n"
        ), "{out}");
    }

    #[test]
    fn install_is_appended_when_the_body_has_none() {
        let out = compose(&package(), Some("# progress-style\n\nUsage docs only.\n"));
        assert!(out.contains("## Install"), "{out}");
        assert!(
            out.contains(
                "progress-style = { git = \"https://github.com/indexable-inc/progress-style\" }"
            ),
            "{out}"
        );
    }

    #[test]
    fn no_changelog_means_no_changelog_line() {
        let pkg = Package {
            has_changelog: false,
            ..package()
        };
        let out = compose(&pkg, None);
        assert!(!out.contains("CHANGELOG.md"), "{out}");
    }

    #[test]
    fn hero_svg_adapts_via_prefers_color_scheme_and_escapes() {
        let svg = hero_svg("a<b", Some("styling & \"quotes\""));
        assert!(svg.contains("@media (prefers-color-scheme: dark)"), "{svg}");
        assert!(svg.contains("text { fill: currentColor; }"), "{svg}");
        assert!(svg.contains("a&lt;b"), "{svg}");
        assert!(svg.contains("styling &amp; &quot;quotes&quot;"), "{svg}");
        assert!(svg.contains("class=\"accent\""), "{svg}");
        assert_eq!(svg, hero_svg("a<b", Some("styling & \"quotes\"")), "deterministic");
    }
}
