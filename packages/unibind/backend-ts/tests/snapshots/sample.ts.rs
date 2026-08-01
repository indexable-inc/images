// struct Row: 
//   field id: 
//   field name: 
//   field tags: 
//   field weights: 
//   field blob: 
//   field home: 

///A sample boundary exercising the ts surface.
#[doc(hidden)]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
mod __unibind_ts_sample_ts {
    #[allow(unused_imports)]
    use super::sample_ts as __unibind_user;
    /// One trailing optional argument on every async export; `undefined`
    /// (or omission) crosses as `None`.
    pub struct __UnibindAbortSignal {
        already_aborted: bool,
        notify: ::std::sync::Arc<::tokio::sync::Notify>,
    }
    impl ::napi::bindgen_prelude::FromNapiValue for __UnibindAbortSignal {
        unsafe fn from_napi_value(
            env: ::napi::sys::napi_env,
            value: ::napi::sys::napi_value,
        ) -> ::napi::Result<Self> {
            let object = unsafe {
                <::napi::bindgen_prelude::Object as ::napi::bindgen_prelude::FromNapiValue>::from_napi_value(
                    env,
                    value,
                )?
            };
            let already_aborted = object.get::<bool>("aborted")?.unwrap_or(false);
            let signal = unsafe {
                <::napi::bindgen_prelude::AbortSignal as ::napi::bindgen_prelude::FromNapiValue>::from_napi_value(
                    env,
                    value,
                )?
            };
            let notify = ::std::sync::Arc::new(::tokio::sync::Notify::new());
            let notifier = ::std::sync::Arc::clone(&notify);
            signal.on_abort(move || notifier.notify_one());
            ::std::result::Result::Ok(Self { already_aborted, notify })
        }
    }
    fn __unibind_aborted() -> ::napi::Error {
        ::napi::Error::new(::napi::Status::Cancelled, "__unibind__:aborted")
    }
    /// Race `future` against `signal`; the shared body of every async
    /// export. The `biased` arm keeps an abort that raced completion
    /// deterministic (the abort wins), and dropping the user future is
    /// the cancellation.
    async fn __unibind_with_abort<__UnibindOutput>(
        signal: ::std::option::Option<__UnibindAbortSignal>,
        future: impl ::std::future::Future<Output = __UnibindOutput>,
    ) -> ::napi::Result<__UnibindOutput> {
        match signal {
            ::std::option::Option::Some(signal) => {
                if signal.already_aborted {
                    return ::std::result::Result::Err(__unibind_aborted());
                }
                ::tokio::select! {
                    biased; () = signal.notify.notified() => {
                    ::std::result::Result::Err(__unibind_aborted()) } value = future =>
                    ::std::result::Result::Ok(value),
                }
            }
            ::std::option::Option::None => ::std::result::Result::Ok(future.await),
        }
    }
    /// A `bigint` JavaScript sent that the declared Rust width cannot
    /// hold. Deliberately not a `__unibind__:` reason: this is a caller
    /// mistake, not a boundary failure the user's error enum declared,
    /// so it surfaces as a plain napi error rather than one of the
    /// generated classes.
    #[allow(dead_code)]
    fn __unibind_bigint_out_of_range(width: &str) -> ::napi::Error {
        ::napi::Error::new(
            ::napi::Status::InvalidArg,
            ::std::format!("bigint does not fit in a Rust `{}`", width),
        )
    }
    /// Narrow a JavaScript `bigint` to `i64`, refusing a value outside the width instead of truncating it.
    #[allow(dead_code)]
    fn __unibind_bigint_to_i64(
        value: ::napi::bindgen_prelude::BigInt,
    ) -> ::napi::Result<i64> {
        let (__unibind_value, __unibind_exact) = value.get_i64();
        if !__unibind_exact {
            return ::std::result::Result::Err(__unibind_bigint_out_of_range("i64"));
        }
        ::std::result::Result::Ok(__unibind_value)
    }
    #[::napi_derive::napi(object, js_name = "SampleRow")]
    pub struct __UnibindRecordRow {
        pub id: ::napi::bindgen_prelude::BigInt,
        #[napi(js_name = "rowLabel")]
        pub name: ::std::string::String,
        pub tags: ::std::vec::Vec<::std::string::String>,
        pub weights: ::std::collections::HashMap<::std::string::String, f64>,
        pub blob: ::std::vec::Vec<u8>,
        pub home: ::std::option::Option<::std::path::PathBuf>,
    }
    #[allow(dead_code)]
    impl __UnibindRecordRow {
        /// The record as JavaScript sees it.
        fn __unibind_from(value: __unibind_user::Row) -> Self {
            Self {
                id: ::napi::bindgen_prelude::BigInt::from(value.id),
                name: value.name,
                tags: value.tags,
                weights: value.weights,
                blob: value.blob,
                home: value.home,
            }
        }
        /// The record as the user's code takes it; a `bigint` outside a
        /// field's declared width is refused here rather than truncated.
        fn __unibind_into(self) -> ::napi::Result<__unibind_user::Row> {
            ::std::result::Result::Ok(__unibind_user::Row {
                id: __unibind_bigint_to_i64(self.id)?,
                name: self.name,
                tags: self.tags,
                weights: self.weights,
                blob: self.blob,
                home: self.home,
            })
        }
    }
    ///Map `SampleError` onto a decodable napi rejection reason, message from `Display`.
    impl ::std::convert::From<__unibind_user::SampleError> for ::napi::Error {
        fn from(error: __unibind_user::SampleError) -> Self {
            let message = ::std::string::ToString::to_string(&error);
            match error {
                __unibind_user::SampleError::StoreGone { .. } => {
                    ::napi::Error::from_reason(
                        ::std::format!(
                            "{}{}", "__unibind__:err:SampleError:StoreGone:", message
                        ),
                    )
                }
                __unibind_user::SampleError::Invalid { .. } => {
                    ::napi::Error::from_reason(
                        ::std::format!(
                            "{}{}", "__unibind__:err:SampleError:Invalid:", message
                        ),
                    )
                }
            }
        }
    }
    ///Fetch rows.
    ///
    ///Docs reach the generated `.d.ts`.
    #[::napi_derive::napi]
    pub fn rows(
        store: ::std::string::String,
        limit: ::std::option::Option<u32>,
        root: ::std::option::Option<::std::string::String>,
    ) -> ::napi::Result<::std::vec::Vec<__UnibindRecordRow>> {
        match __unibind_user::rows(
            store.as_str(),
            limit.unwrap_or(10),
            root.as_deref(),
        ) {
            ::std::result::Result::Ok(value) => {
                ::std::result::Result::Ok(
                    value
                        .into_iter()
                        .map(|__unibind_element| __UnibindRecordRow::__unibind_from(
                            __unibind_element,
                        ))
                        .collect::<::std::vec::Vec<_>>(),
                )
            }
            ::std::result::Result::Err(error) => {
                ::std::result::Result::Err(::napi::Error::from(error))
            }
        }
    }
    #[::napi_derive::napi(js_name = "touchPath")]
    pub fn touch(
        path: ::std::path::PathBuf,
        data: ::napi::bindgen_prelude::Buffer,
        ratio: ::std::option::Option<f64>,
        note: ::std::option::Option<::std::string::String>,
    ) -> bool {
        let value = __unibind_user::touch(
            path.as_path(),
            data.as_ref(),
            ratio.unwrap_or(0.5),
            note.as_deref().unwrap_or("note"),
        );
        value
    }
    ///Wrapping byte sum; `blocking` frees Python's GIL and renders as a
    ///plain sync export for JavaScript.
    #[::napi_derive::napi]
    pub fn checksum(data: ::napi::bindgen_prelude::Buffer) -> u32 {
        let value = __unibind_user::checksum(data.as_ref());
        value
    }
    ///Add, slowly.
    #[::napi_derive::napi]
    pub async fn slow_add(
        a: ::napi::bindgen_prelude::BigInt,
        b: ::napi::bindgen_prelude::BigInt,
        __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
    ) -> ::napi::Result<::napi::bindgen_prelude::BigInt> {
        let a = __unibind_bigint_to_i64(a)?;
        let b = __unibind_bigint_to_i64(b)?;
        let value = __unibind_with_abort(
                __unibind_signal,
                __unibind_user::slow_add(a, b),
            )
            .await?;
        ::std::result::Result::Ok(::napi::bindgen_prelude::BigInt::from(value))
    }
    ///Fetch one row.
    #[::napi_derive::napi]
    pub async fn fetch(
        store: ::std::string::String,
        __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
    ) -> ::napi::Result<__UnibindRecordRow> {
        let value = __unibind_with_abort(__unibind_signal, __unibind_user::fetch(store))
            .await?;
        match value {
            ::std::result::Result::Ok(value) => {
                ::std::result::Result::Ok(__UnibindRecordRow::__unibind_from(value))
            }
            ::std::result::Result::Err(error) => {
                ::std::result::Result::Err(::napi::Error::from(error))
            }
        }
    }
    ///Tail rows as a pull stream.
    #[::napi_derive::napi]
    pub fn tail(store: ::std::string::String) -> __UnibindStreamTail {
        let value = __unibind_user::tail(store.as_str());
        __UnibindStreamTail::__unibind_from(value)
    }
    ///Tail rows once the store opens (an async stream function).
    #[::napi_derive::napi]
    pub async fn tail_later(
        store: ::std::string::String,
        __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
    ) -> ::napi::Result<__UnibindStreamTailLater> {
        let value = __unibind_with_abort(
                __unibind_signal,
                __unibind_user::tail_later(store),
            )
            .await?;
        match value {
            ::std::result::Result::Ok(value) => {
                ::std::result::Result::Ok(
                    __UnibindStreamTailLater::__unibind_from(value),
                )
            }
            ::std::result::Result::Err(error) => {
                ::std::result::Result::Err(::napi::Error::from(error))
            }
        }
    }
    ///Open a counter from a free function (the non-constructor path).
    #[::napi_derive::napi]
    pub fn open_counter(
        start: ::napi::bindgen_prelude::BigInt,
    ) -> ::napi::Result<__UnibindObjectCounter> {
        let start = __unibind_bigint_to_i64(start)?;
        let value = __unibind_user::open_counter(start);
        ::std::result::Result::Ok(__UnibindObjectCounter::__unibind_from(value))
    }
    ///A counter resource.
    #[::napi_derive::napi(js_name = "Counter")]
    pub struct __UnibindObjectCounter {
        inner: ::std::sync::Arc<__unibind_user::Counter>,
        closed: ::std::sync::atomic::AtomicBool,
    }
    impl __UnibindObjectCounter {
        fn __unibind_from(value: __unibind_user::Counter) -> Self {
            Self {
                inner: ::std::sync::Arc::new(value),
                closed: ::std::sync::atomic::AtomicBool::new(false),
            }
        }
    }
    #[::napi_derive::napi]
    impl __UnibindObjectCounter {
        ///Open a counter.
        #[::napi_derive::napi(constructor)]
        pub fn new(
            start: ::std::option::Option<::napi::bindgen_prelude::BigInt>,
        ) -> ::napi::Result<Self> {
            let start = match start {
                ::std::option::Option::Some(start) => __unibind_bigint_to_i64(start)?,
                ::std::option::Option::None => 0,
            };
            match __unibind_user::Counter::new(start) {
                ::std::result::Result::Ok(value) => {
                    ::std::result::Result::Ok(Self::__unibind_from(value))
                }
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(::napi::Error::from(error))
                }
            }
        }
        ///Current value.
        #[::napi_derive::napi]
        pub fn value(&self) -> ::napi::bindgen_prelude::BigInt {
            let value = self.inner.value();
            ::napi::bindgen_prelude::BigInt::from(value)
        }
        ///Add and return the new value.
        #[::napi_derive::napi(js_name = "addSlowly")]
        pub async fn add(
            &self,
            amount: ::napi::bindgen_prelude::BigInt,
            __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
        ) -> ::napi::Result<::napi::bindgen_prelude::BigInt> {
            let amount = __unibind_bigint_to_i64(amount)?;
            let value = __unibind_with_abort(
                    __unibind_signal,
                    {
                        let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
                        async move { __unibind_inner.add(amount).await }
                    },
                )
                .await?;
            match value {
                ::std::result::Result::Ok(value) => {
                    ::std::result::Result::Ok(
                        ::napi::bindgen_prelude::BigInt::from(value),
                    )
                }
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(::napi::Error::from(error))
                }
            }
        }
        ///Every value the counter takes, as a pull stream.
        #[::napi_derive::napi]
        pub fn watch(&self) -> __UnibindStreamCounterWatch {
            let value = self.inner.watch();
            __UnibindStreamCounterWatch::__unibind_from(value)
        }
        ///Labels under `prefix` (an async, throwing, renamed stream
        ///method, whose handle class is scoped by its owner).
        #[::napi_derive::napi(js_name = "tailRows")]
        pub async fn tail(
            &self,
            prefix: ::std::string::String,
            limit: ::std::option::Option<u32>,
            __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
        ) -> ::napi::Result<__UnibindStreamCounterTail> {
            let value = __unibind_with_abort(
                    __unibind_signal,
                    {
                        let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
                        async move {
                            __unibind_inner.tail(prefix, limit.unwrap_or(10)).await
                        }
                    },
                )
                .await?;
            match value {
                ::std::result::Result::Ok(value) => {
                    ::std::result::Result::Ok(
                        __UnibindStreamCounterTail::__unibind_from(value),
                    )
                }
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(::napi::Error::from(error))
                }
            }
        }
        ///Fork a counter: a method handing back another object handle.
        #[::napi_derive::napi]
        pub fn fork(&self) -> __UnibindObjectCounter {
            let value = self.inner.fork();
            __UnibindObjectCounter::__unibind_from(value)
        }
        ///Release the counter.
        #[::napi_derive::napi]
        pub async fn close(&self) -> ::napi::Result<()> {
            let __unibind_first = !self
                .closed
                .swap(true, ::std::sync::atomic::Ordering::SeqCst);
            let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
            if __unibind_first {
                __unibind_inner.close().await;
            }
            ::std::result::Result::Ok(())
        }
    }
    impl ::std::ops::Drop for __UnibindObjectCounter {
        fn drop(&mut self) {
            if !self.closed.load(::std::sync::atomic::Ordering::SeqCst) {
                ::std::eprintln!("unclosed Counter: call close() or use `await using`");
            }
        }
    }
    ///Pull handle over the stream returned by `tail`.
    #[::napi_derive::napi(js_name = "TailStream")]
    pub struct __UnibindStreamTail {
        stream: ::unibind_runtime::PullStream<__unibind_user::Row>,
    }
    impl __UnibindStreamTail {
        fn __unibind_from(
            stream: ::unibind_runtime::UniStream<__unibind_user::Row>,
        ) -> Self {
            Self {
                stream: ::unibind_runtime::PullStream::new(stream),
            }
        }
    }
    #[::napi_derive::napi]
    impl __UnibindStreamTail {
        /// The next element, or `null` once the stream ends or closes.
        #[::napi_derive::napi]
        pub async fn next(&self) -> ::std::option::Option<__UnibindRecordRow> {
            let value = self.stream.next().await?;
            ::std::option::Option::Some(__UnibindRecordRow::__unibind_from(value))
        }
        /// Drop the stream early; a pull in flight resolves `null`, and
        /// the producer sees its stream dropped.
        #[::napi_derive::napi]
        pub fn close(&self) {
            self.stream.close();
        }
    }
    ///Pull handle over the stream returned by `tail_later`.
    #[::napi_derive::napi(js_name = "TailLaterStream")]
    pub struct __UnibindStreamTailLater {
        stream: ::unibind_runtime::PullStream<__unibind_user::Row>,
    }
    impl __UnibindStreamTailLater {
        fn __unibind_from(
            stream: ::unibind_runtime::UniStream<__unibind_user::Row>,
        ) -> Self {
            Self {
                stream: ::unibind_runtime::PullStream::new(stream),
            }
        }
    }
    #[::napi_derive::napi]
    impl __UnibindStreamTailLater {
        /// The next element, or `null` once the stream ends or closes.
        #[::napi_derive::napi]
        pub async fn next(&self) -> ::std::option::Option<__UnibindRecordRow> {
            let value = self.stream.next().await?;
            ::std::option::Option::Some(__UnibindRecordRow::__unibind_from(value))
        }
        /// Drop the stream early; a pull in flight resolves `null`, and
        /// the producer sees its stream dropped.
        #[::napi_derive::napi]
        pub fn close(&self) {
            self.stream.close();
        }
    }
    ///Pull handle over the stream returned by `Counter.watch`.
    #[::napi_derive::napi(js_name = "CounterWatchStream")]
    pub struct __UnibindStreamCounterWatch {
        stream: ::unibind_runtime::PullStream<i64>,
    }
    impl __UnibindStreamCounterWatch {
        fn __unibind_from(stream: ::unibind_runtime::UniStream<i64>) -> Self {
            Self {
                stream: ::unibind_runtime::PullStream::new(stream),
            }
        }
    }
    #[::napi_derive::napi]
    impl __UnibindStreamCounterWatch {
        /// The next element, or `null` once the stream ends or closes.
        #[::napi_derive::napi]
        pub async fn next(
            &self,
        ) -> ::std::option::Option<::napi::bindgen_prelude::BigInt> {
            let value = self.stream.next().await?;
            ::std::option::Option::Some(::napi::bindgen_prelude::BigInt::from(value))
        }
        /// Drop the stream early; a pull in flight resolves `null`, and
        /// the producer sees its stream dropped.
        #[::napi_derive::napi]
        pub fn close(&self) {
            self.stream.close();
        }
    }
    ///Pull handle over the stream returned by `Counter.tail`.
    #[::napi_derive::napi(js_name = "CounterTailRowsStream")]
    pub struct __UnibindStreamCounterTail {
        stream: ::unibind_runtime::PullStream<::std::string::String>,
    }
    impl __UnibindStreamCounterTail {
        fn __unibind_from(
            stream: ::unibind_runtime::UniStream<::std::string::String>,
        ) -> Self {
            Self {
                stream: ::unibind_runtime::PullStream::new(stream),
            }
        }
    }
    #[::napi_derive::napi]
    impl __UnibindStreamCounterTail {
        /// The next element, or `null` once the stream ends or closes.
        #[::napi_derive::napi]
        pub async fn next(&self) -> ::std::option::Option<::std::string::String> {
            let value = self.stream.next().await?;
            ::std::option::Option::Some(value)
        }
        /// Drop the stream early; a pull in flight resolves `null`, and
        /// the producer sees its stream dropped.
        #[::napi_derive::napi]
        pub fn close(&self) {
            self.stream.close();
        }
    }
}

