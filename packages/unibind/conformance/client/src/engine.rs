//!The safe handle over the `conformance` engine cdylib: loading, the IR-hash handshake, and one method per exported function.
use ::stabby::libloading::StabbyLibrary as _;
use crate::error::LoadError;
/// Hex SHA-256 of the interface IR this client was generated from,
/// compared against the engine's handshake symbol at load time.
const EXPECTED_IR_SHA256: &str = "787cf68e644c5704b0ffaf066d43953cf24025d2d355ee1e50b2234e749562c7";
/// A loaded engine with every export resolved and typed.
///
/// The `Engine` keeps the library mapped for its whole lifetime and
/// never exposes unloading: values and futures returned by its
/// methods point into the library's code.
pub struct Engine {
    /// Keeps the library mapped; the resolved pointers below point
    /// into it.
    _library: ::libloading::Library,
    echo_record: extern "C" fn(crate::abi::Sample) -> crate::abi::Sample,
    sum: extern "C" fn(::stabby::vec::Vec<i64>) -> i64,
    fail: extern "C" fn(
        u32,
    ) -> ::stabby::result::Result<u64, crate::abi::ConformanceErrorStable>,
    delayed_double: extern "C" fn(i64) -> ::stabby::future::DynFuture<'static, i64>,
    hang_until_dropped: extern "C" fn() -> ::stabby::future::DynFuture<'static, u64>,
    count_to: extern "C" fn(u64) -> ::unibind_stream::DynStream<'static, u64>,
    reset_cancel_witness: extern "C" fn(),
    cancel_witnessed: extern "C" fn() -> bool,
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
                >(b"unibind_conformance_ir_sha256")
        }
            .map_err(symbol_error("unibind_conformance_ir_sha256"))?;
        let actual: &'static str = ::core::convert::Into::into(ir_sha256());
        if actual != EXPECTED_IR_SHA256 {
            return ::std::result::Result::Err(LoadError::IrHashMismatch {
                expected: EXPECTED_IR_SHA256.to_owned(),
                actual: actual.to_owned(),
            });
        }
        let echo_record = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn(crate::abi::Sample) -> crate::abi::Sample,
                >(b"unibind_conformance_echo_record")
        }
            .map_err(symbol_error("unibind_conformance_echo_record"))?;
        let sum = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn(::stabby::vec::Vec<i64>) -> i64,
                >(b"unibind_conformance_sum")
        }
            .map_err(symbol_error("unibind_conformance_sum"))?;
        let fail = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn(
                        u32,
                    ) -> ::stabby::result::Result<
                        u64,
                        crate::abi::ConformanceErrorStable,
                    >,
                >(b"unibind_conformance_fail")
        }
            .map_err(symbol_error("unibind_conformance_fail"))?;
        let delayed_double = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn(i64) -> ::stabby::future::DynFuture<'static, i64>,
                >(b"unibind_conformance_delayed_double")
        }
            .map_err(symbol_error("unibind_conformance_delayed_double"))?;
        let hang_until_dropped = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn() -> ::stabby::future::DynFuture<'static, u64>,
                >(b"unibind_conformance_hang_until_dropped")
        }
            .map_err(symbol_error("unibind_conformance_hang_until_dropped"))?;
        let count_to = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn(u64) -> ::unibind_stream::DynStream<'static, u64>,
                >(b"unibind_conformance_count_to")
        }
            .map_err(symbol_error("unibind_conformance_count_to"))?;
        let reset_cancel_witness = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn(),
                >(b"unibind_conformance_reset_cancel_witness")
        }
            .map_err(symbol_error("unibind_conformance_reset_cancel_witness"))?;
        let cancel_witnessed = *unsafe {
            library
                .get_stabbied::<
                    extern "C" fn() -> bool,
                >(b"unibind_conformance_cancel_witnessed")
        }
            .map_err(symbol_error("unibind_conformance_cancel_witnessed"))?;
        ::std::result::Result::Ok(Self {
            _library: library,
            echo_record,
            sum,
            fail,
            delayed_double,
            hang_until_dropped,
            count_to,
            reset_cancel_witness,
            cancel_witnessed,
        })
    }
    ///Round-trip a record through the boundary unchanged.
    #[must_use]
    pub fn echo_record(&self, sample: crate::records::Sample) -> crate::records::Sample {
        let sample: crate::abi::Sample = crate::abi::Sample::from(sample);
        let out = (self.echo_record)(sample);
        crate::records::Sample::from(out)
    }
    ///Sum the values.
    #[must_use]
    pub fn sum(&self, values: ::std::vec::Vec<i64>) -> i64 {
        let values: ::stabby::vec::Vec<i64> = values
            .into_iter()
            .collect::<::stabby::vec::Vec<i64>>();
        (self.sum)(values)
    }
    ///Fail with the variant selected by `kind` (0 and 1); anything else
    ///succeeds with the kind echoed back.
    ///
    ///# Errors
    ///
    ///[`ConformanceError::StoreGone`] for 0, [`ConformanceError::Invalid`]
    ///for 1.
    pub fn fail(
        &self,
        kind: u32,
    ) -> ::std::result::Result<u64, crate::error::ConformanceError> {
        match ::std::result::Result::from((self.fail)(kind)) {
            ::std::result::Result::Ok(out) => ::std::result::Result::Ok(out),
            ::std::result::Result::Err(error) => {
                ::std::result::Result::Err(crate::error::ConformanceError::from(error))
            }
        }
    }
    ///Double `x` after yielding once, so the waker crosses the boundary
    ///(the future wakes itself and completes on the second poll).
    ///
    ///The returned [`DelayedDoubleFuture`] resolves the call; dropping it before completion cancels the engine-side future.
    pub fn delayed_double(&self, x: i64) -> DelayedDoubleFuture {
        DelayedDoubleFuture {
            inner: (self.delayed_double)(x),
        }
    }
    ///Never completes; holds a guard whose `Drop` flips the cancellation
    ///witness. Dropping the returned future on the client side must run
    ///that guard through the ABI vtable.
    ///
    ///The returned [`HangUntilDroppedFuture`] resolves the call; dropping it before completion cancels the engine-side future.
    pub fn hang_until_dropped(&self) -> HangUntilDroppedFuture {
        HangUntilDroppedFuture {
            inner: (self.hang_until_dropped)(),
        }
    }
    ///Count `0..limit`, returning `Pending` (with a wake) between items,
    ///so every element exercises the cross-ABI waker path.
    ///
    ///The returned [`CountToStream`] yields the items; dropping it before the end drops the engine-side stream.
    pub fn count_to(&self, limit: u64) -> CountToStream {
        CountToStream {
            inner: (self.count_to)(limit),
        }
    }
    ///Clear the cancellation witness before a new observation.
    pub fn reset_cancel_witness(&self) {
        (self.reset_cancel_witness)();
    }
    ///Whether a `hang_until_dropped` future has been dropped (cancelled)
    ///since the last reset.
    #[must_use]
    pub fn cancel_witnessed(&self) -> bool {
        (self.cancel_witnessed)()
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
///Future returned by [`Engine::hang_until_dropped`]. Dropping it before completion drops the engine-side future through the ABI vtable, cancelling it inside the engine.
#[must_use = "futures do nothing unless polled"]
pub struct HangUntilDroppedFuture {
    inner: ::stabby::future::DynFuture<'static, u64>,
}
impl ::core::future::Future for HangUntilDroppedFuture {
    type Output = u64;
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
///Stream returned by [`Engine::count_to`]. Dropping it before the end drops the engine-side stream through the ABI vtable, cancelling it inside the engine.
#[must_use = "streams do nothing unless polled"]
pub struct CountToStream {
    inner: ::unibind_stream::DynStream<'static, u64>,
}
impl ::futures_core::Stream for CountToStream {
    type Item = u64;
    fn poll_next(
        self: ::core::pin::Pin<&mut Self>,
        context: &mut ::core::task::Context<'_>,
    ) -> ::core::task::Poll<::std::option::Option<Self::Item>> {
        let this = self.get_mut();
        match ::unibind_stream::poll_next(&mut this.inner, context) {
            ::core::task::Poll::Ready(::std::option::Option::Some(out)) => {
                ::core::task::Poll::Ready(::std::option::Option::Some(out))
            }
            ::core::task::Poll::Ready(::std::option::Option::None) => {
                ::core::task::Poll::Ready(::std::option::Option::None)
            }
            ::core::task::Poll::Pending => ::core::task::Poll::Pending,
        }
    }
}
/// Classify a `get_stabbied` failure: a loader error means the
/// symbol is missing, anything else is stabby's type-report
/// mismatch text.
fn symbol_error(
    symbol: &'static str,
) -> impl Fn(::std::boxed::Box<dyn ::std::error::Error + Send + Sync>) -> LoadError {
    move |error| {
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
}
