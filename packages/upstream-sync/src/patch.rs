//! Facts read out of `git format-patch` files (mailbox format).

use std::fs;
use std::path::Path;

use color_eyre::eyre::{Result, WrapErr};
use lazy_regex::regex;

/// The subject line of a patch file (its commit message summary), minus the
/// `Subject:` and `[PATCH ...]` prefixes. Used both as duplicate-search
/// keywords and to describe the patch in the plan.
///
/// # Errors
/// Fails when the patch file is unreadable.
pub fn subject(file: &Path) -> Result<String> {
    let raw =
        fs::read_to_string(file).wrap_err_with(|| format!("cannot read {}", file.display()))?;
    let line = raw
        .lines()
        .find(|l| l.starts_with("Subject:"))
        .unwrap_or("Subject: (none)");
    Ok(regex!(r"^Subject:\s*(\[PATCH[^\]]*\]\s*)?")
        .replace(line, "")
        .into_owned())
}

/// A filesystem/branch-safe slug from a patch file name: drop the `NNNN-`
/// prefix and the `.patch` suffix, keep the descriptive middle.
#[must_use]
pub fn slug(patch: &str) -> String {
    let stem = regex!(r"^[0-9]+-").replace(patch, "");
    let stem = stem.strip_suffix(".patch").unwrap_or(&stem).to_lowercase();
    regex!(r"[^a-z0-9]+")
        .replace_all(&stem, "-")
        .trim_matches('-')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn subject_strips_patch_prefix() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "From 0000 Mon Sep 17 00:00:00 2001").unwrap();
        writeln!(f, "Subject: [PATCH 2/3] fakefix: repair the widget").unwrap();
        assert_eq!(subject(f.path()).unwrap(), "fakefix: repair the widget");
    }

    #[test]
    fn subjectless_patch_reads_none() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(subject(f.path()).unwrap(), "(none)");
    }

    #[test]
    fn slug_is_branch_safe() {
        assert_eq!(
            slug("0003-fix-libstore-don-t-crash.patch"),
            "fix-libstore-don-t-crash"
        );
        assert_eq!(slug("0001-Add.Thing_Now.patch"), "add-thing-now");
    }
}
