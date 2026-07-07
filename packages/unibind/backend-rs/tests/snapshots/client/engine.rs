//!The safe handle over the `sample` engine cdylib: loading, the IR-hash handshake, and one method per exported function.
use ::stabby::libloading::StabbyLibrary as _;
use crate::error::LoadError;
/// Hex SHA-256 of the interface IR this client was generated from,
/// compared against the engine's handshake symbol at load time.
const EXPECTED_IR_SHA256: &str = "c6f40ed284d07ffd4de459686b4fdf633578410c2dffa14379d4ffecc0c8b0c8";
/// A loaded engine with every export resolved and typed.
///
/// The `Engine` keeps the library mapped for its whole lifetime and
/// never exposes unloading: values and futures returned by its
/// methods point into the library's code.
pub struct Engine {
    /// Keeps the library mapped; the resolved pointers below point
    /// into it.
    _library: ::libloading::Library,
    rows: extern "C" fn(
        ::stabby::string::String,
        usize,
        ::stabby::option::Option<::stabby::string::String>,
    ) -> ::stabby::result::Result<
        ::stabby::vec::Vec<crate::abi::Row>,
        crate::abi::SampleErrorStable,
    >,
    touch: extern "C" fn(::stabby::vec::Vec<u8>, ::stabby::vec::Vec<u8>, f64) -> bool,
    reset: extern "C" fn(),
    delayed_double: extern "C" fn(i64) -> ::stabby::future::DynFuture<'static, i64>,
    fetch_row: extern "C" fn(
        ::stabby::string::String,
    ) -> ::stabby::future::DynFuture<
        'static,
        ::stabby::result::Result<crate::abi::Row, crate::abi::SampleErrorStable>,
    >,
    labels: extern "C" fn(
        ::stabby::string::String,
    ) -> ::unibind_stream::DynStream<'static, ::stabby::string::String>,
}
impl Engine {
    /// Load the engine cdylib at `path` and resolve every export.
    ///
    /// The IR-hash handshake runs first: the engine must report
    /// exactly the interface hash this client was generated from.
    /// Nothing loads on a mismatch; there is no fallback.
    ///
    /// # Errors
    ///
    /// [`LoadError`] when the library cannot be opened, a symbol is
    /// missing or fails stabby's structural report check, or the
    /// IR hashes disagree.
    pub fn load(path: &::std::path::Path) -> ::std::result::Result<Self, LoadError> {
        let library = unsafe { ::libloading::Library::new(path) }
            .map_err(|error| {
                LoadError::Dlopen {
                    message: ::std::string::ToString::to_string(&error),
                }
            })?;
        let ir_sha256 = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn() -> ::stabby::str::Str<'static>,
                >(b"unibind_sample_ir_sha256")
        }
            .map_err(|error| symbol_error("unibind_sample_ir_sha256", error.as_ref()))?;
        let actual: &'static str = ::core::convert::Into::into(ir_sha256());
        if actual != EXPECTED_IR_SHA256 {
            return ::std::result::Result::Err(LoadError::IrHashMismatch {
                expected: EXPECTED_IR_SHA256.to_owned(),
                actual: actual.to_owned(),
            });
        }
        let rows = resolve_rows(&library)?;
        let touch = resolve_touch(&library)?;
        let reset = resolve_reset(&library)?;
        let delayed_double = resolve_delayed_double(&library)?;
        let fetch_row = resolve_fetch_row(&library)?;
        let labels = resolve_labels(&library)?;
        ::std::result::Result::Ok(Self {
            _library: library,
            rows,
            touch,
            reset,
            delayed_double,
            fetch_row,
            labels,
        })
    }
    ///Fetch rows.
    ///
    ///Docs travel into the generated client.
    ///
    /// # Errors
    ///
    ///Returns the engine's [`SampleError`](crate::error::SampleError) when the call fails.
    pub fn rows(
        &self,
        store: &str,
        limit: usize,
        root: ::std::option::Option<&str>,
    ) -> ::std::result::Result<
        ::std::vec::Vec<crate::records::Row>,
        crate::error::SampleError,
    > {
        let store: ::stabby::string::String = ::stabby::string::String::from(store);
        let root: ::stabby::option::Option<::stabby::string::String> = root
            .map_or_else(
                ::stabby::option::Option::None,
                |inner| ::stabby::option::Option::Some(
                    ::stabby::string::String::from(inner),
                ),
            );
        match ::std::result::Result::from((self.rows)(store, limit, root)) {
            ::std::result::Result::Ok(out) => {
                ::std::result::Result::Ok(
                    out
                        .into_iter()
                        .map(|item| crate::records::Row::from(item))
                        .collect::<::std::vec::Vec<crate::records::Row>>(),
                )
            }
            ::std::result::Result::Err(error) => {
                ::std::result::Result::Err(crate::error::SampleError::from(error))
            }
        }
    }
    ///Touch a path.
    #[must_use]
    pub fn touch(&self, path: &::std::path::Path, data: &[u8], ratio: f64) -> bool {
        let path: ::stabby::vec::Vec<u8> = ::stabby::vec::Vec::from(
            ::std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()),
        );
        let data: ::stabby::vec::Vec<u8> = ::stabby::vec::Vec::from(data);
        (self.touch)(path, data, ratio)
    }
    ///Reset a counter.
    pub fn reset(&self) {
        (self.reset)();
    }
    ///Double after yielding.
    ///
    ///The returned [`DelayedDoubleFuture`] resolves the call; dropping it before completion cancels the engine-side future.
    pub fn delayed_double(&self, x: i64) -> DelayedDoubleFuture {
        DelayedDoubleFuture {
            inner: (self.delayed_double)(x),
        }
    }
    ///An async call that can fail.
    ///
    ///The returned [`FetchRowFuture`] resolves the call; dropping it before completion cancels the engine-side future.
    pub fn fetch_row(&self, name: ::std::string::String) -> FetchRowFuture {
        let name: ::stabby::string::String = ::stabby::string::String::from(name);
        FetchRowFuture {
            inner: (self.fetch_row)(name),
        }
    }
    ///A stream of labels.
    ///
    ///The returned [`LabelsStream`] yields the items; dropping it before the end drops the engine-side stream.
    pub fn labels(&self, prefix: ::std::string::String) -> LabelsStream {
        let prefix: ::stabby::string::String = ::stabby::string::String::from(prefix);
        LabelsStream {
            inner: (self.labels)(prefix),
        }
    }
}
///Future returned by [`Engine::delayed_double`]. Dropping it before completion drops the engine-side future through the ABI vtable, cancelling it inside the engine.
#[must_use = "futures do nothing unless polled"]
pub struct DelayedDoubleFuture {
    inner: ::stabby::future::DynFuture<'static, i64>,
}
impl ::core::future::Future for DelayedDoubleFuture {
    type Output = i64;
    fn poll(
        self: ::core::pin::Pin<&mut Self>,
        context: &mut ::core::task::Context<'_>,
    ) -> ::core::task::Poll<Self::Output> {
        let this = self.get_mut();
        match ::core::pin::Pin::new(&mut this.inner).poll(context) {
            ::core::task::Poll::Ready(out) => ::core::task::Poll::Ready(out),
            ::core::task::Poll::Pending => ::core::task::Poll::Pending,
        }
    }
}
///Future returned by [`Engine::fetch_row`]. Dropping it before completion drops the engine-side future through the ABI vtable, cancelling it inside the engine.
#[must_use = "futures do nothing unless polled"]
pub struct FetchRowFuture {
    inner: ::stabby::future::DynFuture<
        'static,
        ::stabby::result::Result<crate::abi::Row, crate::abi::SampleErrorStable>,
    >,
}
impl ::core::future::Future for FetchRowFuture {
    type Output = ::std::result::Result<crate::records::Row, crate::error::SampleError>;
    fn poll(
        self: ::core::pin::Pin<&mut Self>,
        context: &mut ::core::task::Context<'_>,
    ) -> ::core::task::Poll<Self::Output> {
        let this = self.get_mut();
        match ::core::pin::Pin::new(&mut this.inner).poll(context) {
            ::core::task::Poll::Ready(out) => {
                ::core::task::Poll::Ready(
                    match ::std::result::Result::from(out) {
                        ::std::result::Result::Ok(out) => {
                            ::std::result::Result::Ok(crate::records::Row::from(out))
                        }
                        ::std::result::Result::Err(error) => {
                            ::std::result::Result::Err(
                                crate::error::SampleError::from(error),
                            )
                        }
                    },
                )
            }
            ::core::task::Poll::Pending => ::core::task::Poll::Pending,
        }
    }
}
///Stream returned by [`Engine::labels`]. Dropping it before the end drops the engine-side stream through the ABI vtable, cancelling it inside the engine.
#[must_use = "streams do nothing unless polled"]
pub struct LabelsStream {
    inner: ::unibind_stream::DynStream<'static, ::stabby::string::String>,
}
impl ::futures_core::Stream for LabelsStream {
    type Item = ::std::string::String;
    fn poll_next(
        self: ::core::pin::Pin<&mut Self>,
        context: &mut ::core::task::Context<'_>,
    ) -> ::core::task::Poll<::std::option::Option<Self::Item>> {
        let this = self.get_mut();
        match ::unibind_stream::poll_next(&mut this.inner, context) {
            ::core::task::Poll::Ready(::std::option::Option::Some(out)) => {
                ::core::task::Poll::Ready(
                    ::std::option::Option::Some(::std::string::String::from(out)),
                )
            }
            ::core::task::Poll::Ready(::std::option::Option::None) => {
                ::core::task::Poll::Ready(::std::option::Option::None)
            }
            ::core::task::Poll::Pending => ::core::task::Poll::Pending,
        }
    }
}
///Resolve `unibind_sample_rows` through stabby's report check.
fn resolve_rows(
    library: &::libloading::Library,
) -> ::std::result::Result<
    extern "C" fn(
        ::stabby::string::String,
        usize,
        ::stabby::option::Option<::stabby::string::String>,
    ) -> ::stabby::result::Result<
        ::stabby::vec::Vec<crate::abi::Row>,
        crate::abi::SampleErrorStable,
    >,
    LoadError,
> {
    let resolved = unsafe {
        library
            .get_stabbied::<
                extern "C" fn(
                    ::stabby::string::String,
                    usize,
                    ::stabby::option::Option<::stabby::string::String>,
                ) -> ::stabby::result::Result<
                    ::stabby::vec::Vec<crate::abi::Row>,
                    crate::abi::SampleErrorStable,
                >,
            >(b"unibind_sample_rows")
    }
        .map_err(|error| symbol_error("unibind_sample_rows", error.as_ref()))?;
    ::std::result::Result::Ok(*resolved)
}
///Resolve `unibind_sample_touch` through stabby's report check.
fn resolve_touch(
    library: &::libloading::Library,
) -> ::std::result::Result<
    extern "C" fn(::stabby::vec::Vec<u8>, ::stabby::vec::Vec<u8>, f64) -> bool,
    LoadError,
> {
    let resolved = unsafe {
        library
            .get_stabbied::<
                extern "C" fn(
                    ::stabby::vec::Vec<u8>,
                    ::stabby::vec::Vec<u8>,
                    f64,
                ) -> bool,
            >(b"unibind_sample_touch")
    }
        .map_err(|error| symbol_error("unibind_sample_touch", error.as_ref()))?;
    ::std::result::Result::Ok(*resolved)
}
///Resolve `unibind_sample_reset` through stabby's report check.
fn resolve_reset(
    library: &::libloading::Library,
) -> ::std::result::Result<extern "C" fn(), LoadError> {
    let resolved = unsafe {
        library.get_stabbied::<extern "C" fn()>(b"unibind_sample_reset")
    }
        .map_err(|error| symbol_error("unibind_sample_reset", error.as_ref()))?;
    ::std::result::Result::Ok(*resolved)
}
///Resolve `unibind_sample_delayed_double` through stabby's report check.
fn resolve_delayed_double(
    library: &::libloading::Library,
) -> ::std::result::Result<
    extern "C" fn(i64) -> ::stabby::future::DynFuture<'static, i64>,
    LoadError,
> {
    let resolved = unsafe {
        library
            .get_stabbied::<
                extern "C" fn(i64) -> ::stabby::future::DynFuture<'static, i64>,
            >(b"unibind_sample_delayed_double")
    }
        .map_err(|error| symbol_error("unibind_sample_delayed_double", error.as_ref()))?;
    ::std::result::Result::Ok(*resolved)
}
///Resolve `unibind_sample_fetch_row` through stabby's report check.
fn resolve_fetch_row(
    library: &::libloading::Library,
) -> ::std::result::Result<
    extern "C" fn(
        ::stabby::string::String,
    ) -> ::stabby::future::DynFuture<
        'static,
        ::stabby::result::Result<crate::abi::Row, crate::abi::SampleErrorStable>,
    >,
    LoadError,
> {
    let resolved = unsafe {
        library
            .get_stabbied::<
                extern "C" fn(
                    ::stabby::string::String,
                ) -> ::stabby::future::DynFuture<
                    'static,
                    ::stabby::result::Result<
                        crate::abi::Row,
                        crate::abi::SampleErrorStable,
                    >,
                >,
            >(b"unibind_sample_fetch_row")
    }
        .map_err(|error| symbol_error("unibind_sample_fetch_row", error.as_ref()))?;
    ::std::result::Result::Ok(*resolved)
}
///Resolve `unibind_sample_labels` through stabby's report check.
fn resolve_labels(
    library: &::libloading::Library,
) -> ::std::result::Result<
    extern "C" fn(
        ::stabby::string::String,
    ) -> ::unibind_stream::DynStream<'static, ::stabby::string::String>,
    LoadError,
> {
    let resolved = unsafe {
        library
            .get_stabbied::<
                extern "C" fn(
                    ::stabby::string::String,
                ) -> ::unibind_stream::DynStream<'static, ::stabby::string::String>,
            >(b"unibind_sample_labels")
    }
        .map_err(|error| symbol_error("unibind_sample_labels", error.as_ref()))?;
    ::std::result::Result::Ok(*resolved)
}
/// Classify a `get_stabbied` failure: a loader error means the
/// symbol is missing, anything else is stabby's type-report
/// mismatch text.
fn symbol_error(
    symbol: &str,
    error: &(dyn ::std::error::Error + Send + Sync + 'static),
) -> LoadError {
    let message = ::std::string::ToString::to_string(&error);
    if error.is::<::libloading::Error>() {
        LoadError::MissingSymbol {
            symbol: symbol.to_owned(),
            message,
        }
    } else {
        LoadError::SignatureMismatch {
            symbol: symbol.to_owned(),
            message,
        }
    }
}

