//! Credential detection over a memory's raw text.
//!
//! Deliberately independent of parsing. A file whose frontmatter does not parse
//! still gets scanned, because a credential in a broken file is the same
//! credential: the first thing anyone does with a parse error is fix the YAML and
//! commit, and being told about the key only after that is being told too late.
//!
//! The rule has a paid-for incident behind it: a live `lin_api_*` key reached at
//! least 200 indexed chunks on this fleet. `validated.how` holds a command line,
//! exactly the shape that leaked, and unlike a transcript a memory is committed
//! on purpose.

/// One line holding what the redaction table reads as a credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    /// 1-based line in the file.
    pub line: usize,
    /// Credential kinds matched, in the order they appear on the line.
    pub kinds: Vec<String>,
}

/// Scan a file's whole text, line by line.
///
/// Detection is `source_meta::sanitize::redact_secrets`: if redaction changes a
/// line, that line holds a credential. Reusing the fleet's table rather than
/// writing a second one means a pattern added there is caught here too.
///
/// Line by line so each finding names a line. A multi-line PEM block still trips
/// on its `BEGIN` marker, whose pattern runs to end of input.
#[must_use]
pub fn scan(contents: &str) -> Vec<Finding> {
    contents
        .lines()
        .enumerate()
        .filter_map(|(offset, line)| {
            let redacted = source_meta::sanitize::redact_secrets(line);
            (redacted != line).then(|| Finding {
                line: offset + 1,
                kinds: kinds_of(&redacted),
            })
        })
        .collect()
}

/// The `[redacted:<kind>]` markers redaction left behind, so a diagnostic can
/// name the credential kind without a second copy of the table.
fn kinds_of(redacted: &str) -> Vec<String> {
    let mut kinds: Vec<String> = Vec::new();
    let mut rest = redacted;
    while let Some(at) = rest.find("[redacted:") {
        rest = &rest[at + "[redacted:".len()..];
        let Some(end) = rest.find(']') else { break };
        let kind = rest[..end].to_owned();
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
        rest = &rest[end..];
    }
    kinds
}

#[cfg(test)]
mod tests {
    use super::scan;

    /// The shape that actually leaked: a bearer token on a command line, which is
    /// what `validated.how` holds.
    #[test]
    fn a_bearer_token_on_a_command_line_is_found_with_its_line_and_kind() {
        let contents = concat!(
            "---\n",
            "tldr: The Linear API needs a bearer token\n",
            "validated:\n",
            "  - at: 2026-07-29T00:00:00Z\n",
            "    by: t\n",
            "    how: 'curl -H \"Authorization: lin_api_abc123\" https://api.linear.app/graphql'\n",
            "    ok: true\n",
            "---\n",
            "Body.\n",
        );
        let findings = scan(contents);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 6);
        assert_eq!(findings[0].kinds, ["linear_api_key"]);
    }

    /// The case the parse gate used to hide: the same token inside frontmatter
    /// that is not valid YAML, because `Authorization: ` makes a plain scalar a
    /// nested mapping. Scanning raw text is what keeps this visible.
    #[test]
    fn a_credential_in_unparseable_frontmatter_is_still_found() {
        let contents = concat!(
            "---\n",
            "tldr: A line\n",
            "validated:\n",
            "  - how: curl -H \"Authorization: lin_api_abc123\" https://api.linear.app\n",
            "---\n",
            "Body.\n",
        );
        let findings = scan(contents);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 4);
        assert_eq!(findings[0].kinds, ["linear_api_key"]);
    }

    #[test]
    fn a_clean_file_has_no_findings() {
        let contents = "---\ntldr: A line\n---\nThe token lives in $LINEAR_API_KEY.\n";
        assert!(scan(contents).is_empty());
    }

    #[test]
    fn several_kinds_on_one_line_are_all_named() {
        let contents = "The keys were AKIA0123456789ABCDEF and lin_api_abc123.\n";
        let findings = scan(contents);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kinds,
            ["aws_access_key_id", "linear_api_key"],
            "in the order the markers appear on the line: {findings:?}"
        );
    }
}
