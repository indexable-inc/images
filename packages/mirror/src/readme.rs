//! The mirror README: a banner at the top declaring the repo a read-only
//! generated mirror (source of truth, exact monorepo tree link, where to file
//! issues/PRs), followed by the package's own README when it has one, or a
//! minimal generated body when it does not.

pub struct Banner<'a> {
    /// Monorepo `owner/name`, e.g. `indexable-inc/index`.
    pub monorepo: &'a str,
    /// Repo-relative package path, e.g. `packages/progress-style`.
    pub package_path: &'a str,
    /// Monorepo commit the tree was generated from (full sha).
    pub commit: &'a str,
    pub crate_name: &'a str,
    pub description: Option<&'a str>,
    /// The mirror repo's own `owner/name`, when known.
    pub mirror_repo: Option<&'a str>,
}

pub fn compose(banner: &Banner<'_>, existing: Option<&str>) -> String {
    let Banner {
        monorepo,
        package_path,
        commit,
        crate_name,
        description,
        mirror_repo,
    } = *banner;
    let short = commit.get(..12).unwrap_or(commit);
    let subject = mirror_repo.map_or_else(
        || "This repository".to_owned(),
        |repo| format!("[`{repo}`](https://github.com/{repo})"),
    );
    let mut out = format!(
        "> [!NOTE]\n\
         > {subject} is a read-only mirror, generated from \
         [`{package_path}`](https://github.com/{monorepo}/tree/{commit}/{package_path}) in \
         [`{monorepo}`](https://github.com/{monorepo}) at commit `{short}`. \
         The monorepo is the source of truth: please open issues and pull requests \
         [there](https://github.com/{monorepo}). This mirror is regenerated automatically; \
         anything pushed directly here will be overwritten.\n\n"
    );
    if let Some(body) = existing {
        out.push_str(body);
    } else {
        out.push_str("# ");
        out.push_str(crate_name);
        out.push('\n');
        if let Some(description) = description {
            out.push('\n');
            out.push_str(description);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banner() -> Banner<'static> {
        Banner {
            monorepo: "indexable-inc/index",
            package_path: "packages/progress-style",
            commit: "0123456789abcdef0123456789abcdef01234567",
            crate_name: "progress-style",
            description: Some("Shared indicatif styling"),
            mirror_repo: Some("indexable-inc/progress-style"),
        }
    }

    #[test]
    fn banner_leads_and_links_the_exact_tree() {
        let out = compose(&banner(), Some("# progress-style\n\nHand-written body.\n"));
        assert!(out.starts_with("> [!NOTE]\n"), "{out}");
        assert!(
            out.contains(
                "https://github.com/indexable-inc/index/tree/0123456789abcdef0123456789abcdef01234567/packages/progress-style"
            ),
            "{out}"
        );
        assert!(out.contains("`0123456789ab`"), "{out}");
        assert!(out.ends_with("Hand-written body.\n"), "{out}");
    }

    #[test]
    fn synthesizes_a_body_when_the_package_has_no_readme() {
        let out = compose(&banner(), None);
        assert!(
            out.contains("# progress-style\n\nShared indicatif styling\n"),
            "{out}"
        );
    }
}
