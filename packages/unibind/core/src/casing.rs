//! The wire spelling of an enum variant.
//!
//! A unit enum's variants cross the boundary as strings, and that string is
//! one value shared by every language: TypeScript compares it as a literal,
//! Python holds it as a `StrEnum` value, and whatever else already reads the
//! field over JSON keeps reading the same word. So the spelling is decided
//! once, here, from the Rust variant name and the enum's `rename_all`.
//!
//! The conventions are serde's, byte for byte, because the platform's own
//! wire formats are produced by `#[serde(rename_all = "...")]` on the same
//! enums. A binding whose spelling disagreed with the JSON the service
//! already emits would be a second vocabulary for one closed set.

/// A naming convention for enum variants on the wire, as
/// `#[unibind::enumeration(rename_all = "...")]` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Casing {
    /// `NotFound` -> `not_found`. The default: it is what
    /// `#[serde(rename_all = "snake_case")]` produces, and what nearly every
    /// closed set in the platform's JSON already spells.
    #[default]
    Snake,
    /// `NotFound` -> `NotFound`; the Rust variant name verbatim.
    Pascal,
    /// `NotFound` -> `notFound`.
    Camel,
    /// `NotFound` -> `NOT_FOUND`.
    ScreamingSnake,
    /// `NotFound` -> `not-found`.
    Kebab,
    /// `NotFound` -> `NOT-FOUND`.
    ScreamingKebab,
    /// `NotFound` -> `notfound`.
    Lower,
    /// `NotFound` -> `NOTFOUND`.
    Upper,
}

/// Every accepted `rename_all` spelling, in the order the rejection message
/// lists them. Public so the diagnostic and the parser cannot drift.
pub const RENAME_ALL_VALUES: [&str; 8] = [
    "snake_case",
    "PascalCase",
    "camelCase",
    "SCREAMING_SNAKE_CASE",
    "kebab-case",
    "SCREAMING-KEBAB-CASE",
    "lowercase",
    "UPPERCASE",
];

impl Casing {
    /// Parse a `rename_all` value; `None` for a spelling that is not one of
    /// [`RENAME_ALL_VALUES`].
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "snake_case" => Self::Snake,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            _ => return None,
        })
    }

    /// The wire spelling of a Rust variant name under this convention.
    #[must_use]
    pub fn apply(self, variant: &str) -> String {
        match self {
            Self::Snake => snake_case(variant),
            Self::Pascal => variant.to_owned(),
            Self::Camel => camel_case(variant),
            Self::ScreamingSnake => screaming_snake_case(variant),
            Self::Kebab => snake_case(variant).replace('_', "-"),
            Self::ScreamingKebab => screaming_snake_case(variant).replace('_', "-"),
            Self::Lower => variant.to_lowercase(),
            Self::Upper => variant.to_uppercase(),
        }
    }
}

/// `NotFound` -> `not_found`, exactly as serde spells it.
///
/// An underscore goes before every uppercase letter but the first, then the
/// whole thing lowercases. Deliberately naive about acronyms (`HTTPGet`
/// becomes `h_t_t_p_get`) so the output matches
/// `#[serde(rename_all = "snake_case")]` on the same enum rather than being
/// cleverer than the JSON it has to agree with.
#[must_use]
pub fn snake_case(variant: &str) -> String {
    let mut out = String::with_capacity(variant.len());
    for (index, character) in variant.char_indices() {
        if index > 0 && character.is_uppercase() {
            out.push('_');
        }
        out.extend(character.to_lowercase());
    }
    out
}

/// `NotFound` -> `NOT_FOUND`; [`snake_case`] uppercased.
///
/// Also the Python member identifier, which is that language's convention
/// for an enum member whatever the value spells.
#[must_use]
pub fn screaming_snake_case(variant: &str) -> String {
    snake_case(variant).to_uppercase()
}

/// `NotFound` -> `notFound`.
///
/// The Rust variant name with its first character lowercased, which is what
/// serde does.
#[must_use]
pub fn camel_case(variant: &str) -> String {
    let mut chars = variant.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().collect::<String>() + chars.as_str()
    })
}

/// `forward_port` -> `forwardPort`.
///
/// napi's own conversion, applied to every unrenamed function, method,
/// argument and record field, and the convention the JVM backend's Java
/// names follow too.
///
/// Distinct from [`camel_case`], which lowercases one leading character to
/// spell a *wire* value the way serde does. This one is about identifiers a
/// caller types, so it consumes the separators.
#[must_use]
pub fn lower_camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper_next = false;
    for character in name.chars() {
        if character == '_' {
            upper_next = !out.is_empty();
        } else if upper_next {
            out.extend(character.to_uppercase());
            upper_next = false;
        } else {
            out.push(character);
        }
    }
    out
}
