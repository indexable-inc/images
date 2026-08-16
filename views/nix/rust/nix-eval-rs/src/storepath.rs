//! Deciding whether a string is a store path, without a store.
//!
//! cppnix asks `StoreDirConfig::isStorePath`, which sounds like a store
//! question and is not: it canonicalises the string, checks the parent
//! directory against `storeDir`, and validates the base name's shape
//! (`store-dir-config.cc:9`, `path.cc:43`). Nothing is read. The store
//! directory is already handed over (`ixe_set_store_dir`), so this is a pure
//! function of two strings and belongs here rather than behind [`crate::host`].
//!
//! What *is* a store question is `ensurePath`, which cppnix calls next in
//! `builtins.appendContext`; that one leaves through `Host`.

/// cppnix's `StorePath::HashLen`: the nix32 hash before the dash.
const HASH_LEN: usize = 32;

/// cppnix's `StorePath::MaxPathLen`, applied to the name after the dash.
const MAX_NAME_LEN: usize = 211;

/// `canonPath` (`file-path-impl.hh:129`) with symlink resolution off, which
/// is the setting `parseStorePath` uses: a pure string walk that collapses
/// repeated separators, drops `.`, and pops the previous component on `..`.
///
/// `None` for a relative path, where cppnix throws and
/// `maybeParseStorePath` turns the throw into "not a store path".
fn canon_path(path: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    let mut out = String::with_capacity(path.len());
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if let Some(i) = out.rfind('/') {
                    out.truncate(i);
                }
            }
            other => {
                out.push('/');
                out.push_str(other);
            }
        }
    }
    Some(out)
}

/// cppnix's `checkName` (`path.cc:8`): what may appear after the hash, and
/// why not, in cppnix's own words.
///
/// The message matters and is not decoration. `builtins.fetchurl` puts it
/// inside a larger error -- "invalid store path name when fetching URL '%s':
/// %s. %s" -- so a caller who mistyped a `name` attribute is told which rule
/// they broke. Returning a bool here and rebuilding the prose at the call
/// site would be a second copy of the rule to drift from this one.
///
/// # Errors
///
/// The message cppnix's `BadStorePathName` carries, for a name it rejects.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name must not be empty".to_owned());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "name '{name}' must be no longer than {MAX_NAME_LEN} characters"
        ));
    }
    // The first dash-separated component may not be "." or "..", which is
    // what would let a store path's name escape the directory it names.
    // cppnix spells this out byte by byte (`path.cc:16`) and the four cases
    // it enumerates -- ".", "..", ".-...", "..-..." -- are exactly that, but
    // it raises two *different* messages across them, so the structure is
    // mirrored rather than collapsed into one component test.
    let b = name.as_bytes();
    if b.first() == Some(&b'.') {
        if b.len() == 1 {
            return Err(format!("name '{name}' is not valid"));
        }
        if b.get(1) == Some(&b'-') {
            return Err(format!(
                "name '{name}' is not valid: first dash-separated component must not be '.'"
            ));
        }
        if b.get(1) == Some(&b'.') {
            if b.len() == 2 {
                return Err(format!("name '{name}' is not valid"));
            }
            if b.get(2) == Some(&b'-') {
                return Err(format!(
                    "name '{name}' is not valid: first dash-separated component must not be '..'"
                ));
            }
        }
    }
    for c in name.chars() {
        let ok = c.is_ascii_digit()
            || c.is_ascii_lowercase()
            || c.is_ascii_uppercase()
            || matches!(c, '+' | '-' | '.' | '_' | '?' | '=');
        if !ok {
            return Err(format!("name '{name}' contains illegal character '{c}'"));
        }
    }
    Ok(())
}

/// [`check_name`] as a predicate, for the store-path parser, which has no
/// message to carry.
fn name_is_valid(name: &str) -> bool {
    check_name(name).is_ok()
}

/// cppnix's `StorePath::StorePath(std::string_view)` (`path.cc:43`) as a
/// predicate: 32 nix32 hash characters, then a name.
///
/// The byte between the two is deliberately unchecked, because cppnix does
/// not check it either: it validates `hashPart()` (the first 32 bytes) and
/// `name()` (everything from byte 33 on), and byte 32 falls between the two.
/// So `<32 hash chars>_foo` parses there and parses here. Mirrored rather
/// than tightened, since the whole point of this function is to answer what
/// cppnix answers.
fn base_name_is_valid(base_name: &str) -> bool {
    if base_name.len() < HASH_LEN + 1 {
        return false;
    }
    let Some(hash) = base_name.get(..HASH_LEN) else {
        return false;
    };
    let hash_ok = hash.bytes().all(|c| {
        !matches!(c, b'e' | b'o' | b'u' | b't') && (c.is_ascii_digit() || c.is_ascii_lowercase())
    });
    let Some(name) = base_name.get(HASH_LEN + 1..) else {
        return false;
    };
    hash_ok && name_is_valid(name)
}

/// The base name of `path` when it is a path directly inside `store_dir`
/// whose shape a `StorePath` accepts, else `None`.
///
/// Owned rather than borrowed from the argument, because the name is taken
/// from the canonicalised form: `/nix/store/<base>/` and `/nix/./store/<base>`
/// are both that store path and neither ends in the bytes of `base`.
///
/// cppnix's `maybeParseStorePath` (`store-dir-config.cc:28`).
#[must_use]
pub fn parse_store_path(store_dir: &str, path: &str) -> Option<String> {
    let canon = canon_path(path)?;
    let (parent, base) = canon.rsplit_once('/')?;
    if parent != store_dir || !base_name_is_valid(base) {
        return None;
    }
    Some(base.to_owned())
}

/// cppnix's `StoreDirConfig::isStorePath`.
#[must_use]
pub fn is_store_path(store_dir: &str, path: &str) -> bool {
    parse_store_path(store_dir, path).is_some()
}

/// cppnix's `StorePath::isDerivation`: the name after the hash ends in
/// `.drv`. Checked against the whole path because the hash part cannot
/// contain a `.` -- it is nix32 -- so a path ending in `.drv` can only be
/// ending in a name that does.
#[must_use]
pub fn is_derivation(path: &str) -> bool {
    path.ends_with(".drv")
}

#[cfg(test)]
mod tests {
    use super::{canon_path, is_derivation, is_store_path, parse_store_path};

    /// A real store path, taken from the goldens in `drvpath`.
    const P: &str = "/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-hello.drv";

    #[test]
    fn a_real_store_path_parses() {
        assert_eq!(
            parse_store_path("/nix/store", P).as_deref(),
            Some("x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-hello.drv")
        );
        // A trailing slash and a redundant component name the same path, and
        // the name comes from the canonical form rather than from the bytes.
        assert_eq!(
            parse_store_path("/nix/store", &format!("{P}/")).as_deref(),
            Some("x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-hello.drv")
        );
        assert!(is_store_path("/nix/store", P));
        assert!(is_derivation(P));
    }

    #[test]
    fn the_store_directory_is_part_of_the_answer() {
        // The same bytes are not a store path of a store rooted elsewhere,
        // which is the reason the directory is handed over rather than
        // assumed.
        assert!(!is_store_path("/other/store", P));
    }

    #[test]
    fn a_path_below_the_store_is_not_a_store_path() {
        assert!(!is_store_path(
            "/nix/store",
            "/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-hello.drv/inner"
        ));
    }

    #[test]
    fn canonicalisation_happens_before_the_parent_check() {
        assert_eq!(
            canon_path("/nix//store/./a/../b"),
            Some("/nix/store/b".to_owned())
        );
        assert!(is_store_path(
            "/nix/store",
            "/nix/./store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-hello.drv"
        ));
        assert_eq!(canon_path("relative/x"), None);
        assert!(!is_store_path(
            "/nix/store",
            "x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-hello.drv"
        ));
    }

    #[test]
    fn a_bad_hash_or_name_is_refused() {
        // 'e', 'o', 'u' and 't' are not nix32 characters.
        assert!(!is_store_path(
            "/nix/store",
            "/nix/store/e0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-hello.drv"
        ));
        // Too short to hold a hash and a name.
        assert!(!is_store_path("/nix/store", "/nix/store/abc-x"));
        // An empty name.
        assert!(!is_store_path(
            "/nix/store",
            "/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-"
        ));
        // A name whose first dash-separated component is "..".
        assert!(!is_store_path(
            "/nix/store",
            "/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd..-x"
        ));
        // An illegal character in the name.
        assert!(!is_store_path(
            "/nix/store",
            "/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-he!lo"
        ));
    }

    /// The messages [`check_name`] carries, which `builtins.fetchurl` puts
    /// in front of a user. cppnix raises two different ones across the four
    /// leading-dot cases, so a single "first component" test would report
    /// half of them with the wrong wording.
    #[test]
    fn a_rejected_name_says_which_rule_it_broke() {
        use super::check_name;
        assert_eq!(check_name("hello-2.12.3.tar.gz"), Ok(()));
        // `.foo` is fine: only "." and ".." as the first dash-separated
        // component are not.
        assert_eq!(check_name(".foo"), Ok(()));
        assert_eq!(check_name("..foo"), Ok(()));

        assert_eq!(check_name(""), Err("name must not be empty".to_owned()));
        assert_eq!(check_name("."), Err("name '.' is not valid".to_owned()));
        assert_eq!(check_name(".."), Err("name '..' is not valid".to_owned()));
        assert_eq!(
            check_name(".-x"),
            Err(
                "name '.-x' is not valid: first dash-separated component must not be '.'"
                    .to_owned()
            )
        );
        assert_eq!(
            check_name("..-x"),
            Err(
                "name '..-x' is not valid: first dash-separated component must not be '..'"
                    .to_owned()
            )
        );
        assert_eq!(
            check_name("a b"),
            Err("name 'a b' contains illegal character ' '".to_owned())
        );
        assert_eq!(
            check_name("a:b"),
            Err("name 'a:b' contains illegal character ':'".to_owned())
        );
        let long = "a".repeat(super::MAX_NAME_LEN + 1);
        assert_eq!(
            check_name(&long),
            Err(format!(
                "name '{long}' must be no longer than 211 characters"
            ))
        );
    }

    /// cppnix validates the hash and the name and skips the byte between
    /// them, so this parses there. Asserted so that "tightening" it later
    /// shows up as a deliberate divergence rather than a silent one.
    #[test]
    fn the_separator_byte_is_not_checked_because_cppnix_does_not_check_it() {
        assert!(is_store_path(
            "/nix/store",
            "/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd_hello"
        ));
    }
}
