//!The raw ABI surface shared with the engine: stable mirror structs and error carriers, token-identical to the types the engine's generated glue compiles. stabby's report check verifies the match structurally at load time.
///ABI-stable mirror of `Sample`, field for field in declaration order.
#[::stabby::stabby(no_opt, module = "unibind::conformance")]
pub struct Sample {
    ///Deliberately awkward leading bool.
    pub flag: bool,
    ///Identifier.
    pub id: u64,
    ///Display name.
    pub name: ::stabby::string::String,
    ///Optional note.
    pub note: ::stabby::option::Option<::stabby::string::String>,
    ///Plain values.
    pub values: ::stabby::vec::Vec<i64>,
    ///Keyed weights.
    pub weights: ::stabby::vec::Vec<
        ::stabby::tuple::Tuple2<::stabby::string::String, i64>,
    >,
    ///A nested record.
    pub inner: Inner,
}
impl ::core::convert::From<crate::records::Sample> for Sample {
    fn from(value: crate::records::Sample) -> Self {
        Self {
            flag: value.flag,
            id: value.id,
            name: ::stabby::string::String::from(value.name),
            note: match value.note {
                ::std::option::Option::Some(inner) => {
                    ::stabby::option::Option::Some(::stabby::string::String::from(inner))
                }
                ::std::option::Option::None => ::stabby::option::Option::None(),
            },
            values: value.values.into_iter().collect::<::stabby::vec::Vec<i64>>(),
            weights: value
                .weights
                .into_iter()
                .map(|(key, value)| ::stabby::tuple::Tuple2::from((
                    ::stabby::string::String::from(key),
                    value,
                )))
                .collect::<
                    ::stabby::vec::Vec<
                        ::stabby::tuple::Tuple2<::stabby::string::String, i64>,
                    >,
                >(),
            inner: Inner::from(value.inner),
        }
    }
}
impl ::core::convert::From<Sample> for crate::records::Sample {
    fn from(value: Sample) -> Self {
        Self {
            flag: value.flag,
            id: value.id,
            name: ::std::string::String::from(value.name),
            note: value
                .note
                .match_owned(
                    |inner| ::std::option::Option::Some(
                        ::std::string::String::from(inner),
                    ),
                    || ::std::option::Option::None,
                ),
            values: value.values.into_iter().collect::<::std::vec::Vec<i64>>(),
            weights: value
                .weights
                .into_iter()
                .map(|pair| {
                    let (key, value): (::stabby::string::String, i64) = pair.into();
                    (::std::string::String::from(key), value)
                })
                .collect::<::std::collections::HashMap<::std::string::String, i64>>(),
            inner: crate::records::Inner::from(value.inner),
        }
    }
}
///ABI-stable mirror of `Inner`, field for field in declaration order.
#[::stabby::stabby(no_opt, module = "unibind::conformance")]
pub struct Inner {
    ///A label.
    pub label: ::stabby::string::String,
    ///A ratio.
    pub ratio: f64,
}
impl ::core::convert::From<crate::records::Inner> for Inner {
    fn from(value: crate::records::Inner) -> Self {
        Self {
            label: ::stabby::string::String::from(value.label),
            ratio: value.ratio,
        }
    }
}
impl ::core::convert::From<Inner> for crate::records::Inner {
    fn from(value: Inner) -> Self {
        Self {
            label: ::std::string::String::from(value.label),
            ratio: value.ratio,
        }
    }
}
///ABI-stable carrier for `ConformanceError`: variant index in declaration order plus the variant's `Display` text.
#[::stabby::stabby(no_opt, module = "unibind::conformance")]
pub struct ConformanceErrorStable {
    /// Index of the variant, in declaration order.
    pub variant: u32,
    /// The variant's `Display` text.
    pub message: ::stabby::string::String,
}
