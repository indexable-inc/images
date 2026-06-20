//! Small string-case helpers.

/// Convert a `CamelCase`, `kebab-case`, or space-separated name into `snake_case`.
///
/// Word boundaries are detected at:
/// - any run of non-alphanumeric characters (spaces, `-`, `_`, punctuation);
/// - a lowercase letter or digit followed by an uppercase letter
///   (`fooBar` -> `foo_bar`);
/// - the end of an acronym, i.e. an uppercase letter that is both preceded by an
///   uppercase letter and followed by a lowercase one (`HTTPServer` ->
///   `http_server`).
///
/// Separator runs collapse and leading/trailing separators are dropped, so the
/// result never contains a doubled or edge underscore. Non-ASCII letters are
/// lowercased per Unicode rules.
///
/// # Examples
/// ```
/// use text_utils::to_snake_case;
///
/// assert_eq!(to_snake_case("CamelCase"), "camel_case");
/// assert_eq!(to_snake_case("kebab-case"), "kebab_case");
/// assert_eq!(to_snake_case("space separated"), "space_separated");
/// assert_eq!(to_snake_case("HTTPServer"), "http_server");
/// assert_eq!(to_snake_case("getHTTPResponseCode"), "get_http_response_code");
/// ```
#[must_use]
pub fn to_snake_case(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    // Headroom for the underscores we insert at word boundaries.
    let mut out = String::with_capacity(input.len() + input.len() / 2);

    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            // A run of separators becomes a single boundary, never a leading or
            // doubled underscore.
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            continue;
        }

        if c.is_uppercase() {
            let prev = i.checked_sub(1).map(|p| chars[p]);
            // `fooBar` / `v2X`: a lowercase letter or digit precedes this upper.
            let after_lower_or_digit =
                prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit());
            // `HTTPServer`: an acronym ends where Upper is followed by lower.
            let acronym_end = prev.is_some_and(char::is_uppercase)
                && chars.get(i + 1).is_some_and(|n| n.is_lowercase());

            if (after_lower_or_digit || acronym_end) && !out.is_empty() && !out.ends_with('_')
            {
                out.push('_');
            }
        }

        out.extend(c.to_lowercase());
    }

    // A trailing separator run leaves one underscore; drop it.
    if out.ends_with('_') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::to_snake_case;

    #[test]
    fn camel_case() {
        assert_eq!(to_snake_case("CamelCase"), "camel_case");
        assert_eq!(to_snake_case("camelCase"), "camel_case");
        assert_eq!(to_snake_case("Camel"), "camel");
    }

    #[test]
    fn kebab_case() {
        assert_eq!(to_snake_case("kebab-case"), "kebab_case");
        assert_eq!(to_snake_case("a-b-c"), "a_b_c");
    }

    #[test]
    fn space_separated() {
        assert_eq!(to_snake_case("space separated"), "space_separated");
        assert_eq!(to_snake_case("Title Case Name"), "title_case_name");
    }

    #[test]
    fn acronyms() {
        assert_eq!(to_snake_case("HTTPServer"), "http_server");
        assert_eq!(to_snake_case("getHTTPResponseCode"), "get_http_response_code");
        assert_eq!(to_snake_case("XMLHttpRequest"), "xml_http_request");
        assert_eq!(to_snake_case("ABC"), "abc");
    }

    #[test]
    fn digits() {
        assert_eq!(to_snake_case("version2Point0"), "version2_point0");
        assert_eq!(to_snake_case("v2X"), "v2_x");
    }

    #[test]
    fn mixed_separators() {
        assert_eq!(to_snake_case("Some-Mixed Case_String"), "some_mixed_case_string");
    }

    #[test]
    fn collapses_and_trims_separators() {
        assert_eq!(to_snake_case("  Multiple   Spaces  "), "multiple_spaces");
        assert_eq!(to_snake_case("--leading-and-trailing--"), "leading_and_trailing");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
    }

    #[test]
    fn edge_cases() {
        assert_eq!(to_snake_case(""), "");
        assert_eq!(to_snake_case("---"), "");
        assert_eq!(to_snake_case("a"), "a");
        assert_eq!(to_snake_case("A"), "a");
    }

    #[test]
    fn unicode_lowercasing() {
        assert_eq!(to_snake_case("Été"), "été");
        assert_eq!(to_snake_case("naïveCase"), "naïve_case");
    }
}
