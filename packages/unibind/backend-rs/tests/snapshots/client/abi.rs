//!The raw ABI surface shared with the engine.
//!
//!Stable mirror structs and error carriers, token-identical to the types the engine's generated glue compiles; stabby's report check verifies the match structurally at load time.
///ABI-stable mirror of `Row`, field for field in declaration order.
#[::stabby::stabby(no_opt, module = "unibind::sample")]
pub struct Row {
    ///Awkwardly-placed bool.
    pub flag: bool,
    ///Identifier.
    pub id: u64,
    pub name: ::stabby::string::String,
    pub tags: ::stabby::vec::Vec<::stabby::string::String>,
    pub weights: ::stabby::vec::Vec<
        ::stabby::tuple::Tuple2<::stabby::string::String, f64>,
    >,
    pub blob: ::stabby::vec::Vec<u8>,
    pub home: ::stabby::option::Option<::stabby::vec::Vec<u8>>,
    pub nested: ::stabby::option::Option<::stabby::vec::Vec<Inner>>,
    pub inner: Inner,
}
impl ::core::convert::From<crate::records::Row> for Row {
    fn from(value: crate::records::Row) -> Self {
        Self {
            flag: value.flag,
            id: value.id,
            name: ::stabby::string::String::from(value.name),
            tags: value
                .tags
                .into_iter()
                .map(|item| ::stabby::string::String::from(item))
                .collect::<::stabby::vec::Vec<::stabby::string::String>>(),
            weights: value
                .weights
                .into_iter()
                .map(|(key, value)| ::stabby::tuple::Tuple2::from((
                    ::stabby::string::String::from(key),
                    value,
                )))
                .collect::<
                    ::stabby::vec::Vec<
                        ::stabby::tuple::Tuple2<::stabby::string::String, f64>,
                    >,
                >(),
            blob: value.blob.into_iter().collect::<::stabby::vec::Vec<u8>>(),
            home: value
                .home
                .map_or_else(
                    ::stabby::option::Option::None,
                    |inner| ::stabby::option::Option::Some(
                        ::std::os::unix::ffi::OsStringExt::into_vec(
                                inner.into_os_string(),
                            )
                            .into_iter()
                            .collect::<::stabby::vec::Vec<u8>>(),
                    ),
                ),
            nested: value
                .nested
                .map_or_else(
                    ::stabby::option::Option::None,
                    |inner| ::stabby::option::Option::Some(
                        inner
                            .into_iter()
                            .map(|item| Inner::from(item))
                            .collect::<::stabby::vec::Vec<Inner>>(),
                    ),
                ),
            inner: Inner::from(value.inner),
        }
    }
}
impl ::core::convert::From<Row> for crate::records::Row {
    fn from(value: Row) -> Self {
        Self {
            flag: value.flag,
            id: value.id,
            name: ::std::string::String::from(value.name),
            tags: value
                .tags
                .into_iter()
                .map(|item| ::std::string::String::from(item))
                .collect::<::std::vec::Vec<::std::string::String>>(),
            weights: value
                .weights
                .into_iter()
                .map(|pair| {
                    let (key, value): (::stabby::string::String, f64) = pair.into();
                    (::std::string::String::from(key), value)
                })
                .collect::<::std::collections::HashMap<::std::string::String, f64>>(),
            blob: value.blob.into_iter().collect::<::std::vec::Vec<u8>>(),
            home: value
                .home
                .match_owned(
                    |inner| ::std::option::Option::Some(
                        ::std::path::PathBuf::from(
                            ::std::os::unix::ffi::OsStringExt::from_vec(
                                inner.into_iter().collect::<::std::vec::Vec<u8>>(),
                            ),
                        ),
                    ),
                    || ::std::option::Option::None,
                ),
            nested: value
                .nested
                .match_owned(
                    |inner| ::std::option::Option::Some(
                        inner
                            .into_iter()
                            .map(|item| crate::records::Inner::from(item))
                            .collect::<::std::vec::Vec<crate::records::Inner>>(),
                    ),
                    || ::std::option::Option::None,
                ),
            inner: crate::records::Inner::from(value.inner),
        }
    }
}
///ABI-stable mirror of `Inner`, field for field in declaration order.
#[::stabby::stabby(no_opt, module = "unibind::sample")]
pub struct Inner {
    pub label: ::stabby::string::String,
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
///ABI-stable carrier for `SampleError`: variant index in declaration order plus the variant's `Display` text.
#[::stabby::stabby(no_opt, module = "unibind::sample")]
pub struct SampleErrorStable {
    /// Index of the variant, in declaration order.
    pub variant: u32,
    /// The variant's `Display` text.
    pub message: ::stabby::string::String,
}

