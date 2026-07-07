#[doc(hidden)]
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    unused_qualifications,
    improper_ctypes_definitions
)]
mod __unibind_rs_sample {
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
    impl ::core::convert::From<super::sample::Row> for Row {
        fn from(value: super::sample::Row) -> Self {
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
    impl ::core::convert::From<Row> for super::sample::Row {
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
                    .collect::<
                        ::std::collections::HashMap<::std::string::String, f64>,
                    >(),
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
                                .map(|item| super::sample::Inner::from(item))
                                .collect::<::std::vec::Vec<super::sample::Inner>>(),
                        ),
                        || ::std::option::Option::None,
                    ),
                inner: super::sample::Inner::from(value.inner),
            }
        }
    }
    ///ABI-stable mirror of `Inner`, field for field in declaration order.
    #[::stabby::stabby(no_opt, module = "unibind::sample")]
    pub struct Inner {
        pub label: ::stabby::string::String,
        pub ratio: f64,
    }
    impl ::core::convert::From<super::sample::Inner> for Inner {
        fn from(value: super::sample::Inner) -> Self {
            Self {
                label: ::stabby::string::String::from(value.label),
                ratio: value.ratio,
            }
        }
    }
    impl ::core::convert::From<Inner> for super::sample::Inner {
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
    impl ::core::convert::From<super::sample::SampleError> for SampleErrorStable {
        fn from(error: super::sample::SampleError) -> Self {
            let message = ::stabby::string::String::from(
                ::std::string::ToString::to_string(&error),
            );
            let variant = match error {
                super::sample::SampleError::StoreGone { .. } => 0u32,
                super::sample::SampleError::Invalid { .. } => 1u32,
            };
            Self { variant, message }
        }
    }
    #[::stabby::export]
    pub extern "C" fn unibind_sample_rows(
        store: ::stabby::string::String,
        limit: usize,
        root: ::stabby::option::Option<::stabby::string::String>,
    ) -> ::stabby::result::Result<::stabby::vec::Vec<Row>, SampleErrorStable> {
        let store: ::std::string::String = ::std::string::String::from(store);
        let limit: usize = limit;
        let root: ::std::option::Option<::std::string::String> = root
            .match_owned(
                |inner| ::std::option::Option::Some(::std::string::String::from(inner)),
                || ::std::option::Option::None,
            );
        match super::sample::rows(&store, limit, root.as_deref()) {
            ::std::result::Result::Ok(out) => {
                ::stabby::result::Result::Ok(
                    out
                        .into_iter()
                        .map(|item| Row::from(item))
                        .collect::<::stabby::vec::Vec<Row>>(),
                )
            }
            ::std::result::Result::Err(error) => {
                ::stabby::result::Result::Err(SampleErrorStable::from(error))
            }
        }
    }
    #[::stabby::export]
    pub extern "C" fn unibind_sample_touch(
        path: ::stabby::vec::Vec<u8>,
        data: ::stabby::vec::Vec<u8>,
        ratio: f64,
    ) -> bool {
        let path: ::std::path::PathBuf = ::std::path::PathBuf::from(
            ::std::os::unix::ffi::OsStringExt::from_vec(
                path.into_iter().collect::<::std::vec::Vec<u8>>(),
            ),
        );
        let data: ::std::vec::Vec<u8> = data
            .into_iter()
            .collect::<::std::vec::Vec<u8>>();
        let ratio: f64 = ratio;
        let out: bool = super::sample::touch(&path, &data, ratio);
        out
    }
    #[::stabby::export]
    pub extern "C" fn unibind_sample_reset() {
        super::sample::reset()
    }
    #[::stabby::export]
    pub extern "C" fn unibind_sample_delayed_double(
        x: i64,
    ) -> ::stabby::future::DynFuture<'static, i64> {
        let x: i64 = x;
        ::stabby::boxed::Box::new(async move {
                let out: i64 = super::sample::delayed_double(x).await;
                out
            })
            .into()
    }
    #[::stabby::export]
    pub extern "C" fn unibind_sample_fetch_row(
        name: ::stabby::string::String,
    ) -> ::stabby::future::DynFuture<
        'static,
        ::stabby::result::Result<Row, SampleErrorStable>,
    > {
        let name: ::std::string::String = ::std::string::String::from(name);
        ::stabby::boxed::Box::new(async move {
                match super::sample::fetch_row(name).await {
                    ::std::result::Result::Ok(out) => {
                        ::stabby::result::Result::Ok(Row::from(out))
                    }
                    ::std::result::Result::Err(error) => {
                        ::stabby::result::Result::Err(SampleErrorStable::from(error))
                    }
                }
            })
            .into()
    }
    #[::stabby::export]
    pub extern "C" fn unibind_sample_labels(
        prefix: ::stabby::string::String,
    ) -> ::unibind_stream::DynStream<'static, ::stabby::string::String> {
        let prefix: ::std::string::String = ::std::string::String::from(prefix);
        ::stabby::boxed::Box::new(
                ::unibind_stream::StreamAdapter::new(super::sample::labels(prefix)),
            )
            .into()
    }
    #[::stabby::export]
    pub extern "C" fn unibind_sample_ir_sha256() -> ::stabby::str::Str<'static> {
        ::stabby::str::Str::from(
            "c6f40ed284d07ffd4de459686b4fdf633578410c2dffa14379d4ffecc0c8b0c8",
        )
    }
}

