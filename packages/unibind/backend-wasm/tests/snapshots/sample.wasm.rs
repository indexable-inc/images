// struct Row: 
//   field id: 
//   field name: 
//   field tags: 
//   field weights: 
//   field blob: 
//   field home: 

///A sample boundary exercising the wasm surface.
#[doc(hidden)]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
mod __unibind_wasm_sample_wasm {
    #[allow(unused_imports)]
    use super::sample_wasm as __unibind_user;
    #[::wasm_bindgen::prelude::wasm_bindgen]
    extern "C" {
        /// The `AbortSignal` surface the bridge uses. `structural` reads
        /// each member off the object it was handed instead of through the
        /// class, so a signal from another realm still works.
        #[wasm_bindgen(js_name = "AbortSignal")]
        pub type __UnibindWasmAbortSignal;
        #[wasm_bindgen(method, getter, structural)]
        fn aborted(this: &__UnibindWasmAbortSignal) -> bool;
        #[wasm_bindgen(method, structural, js_name = "addEventListener")]
        fn add_event_listener(
            this: &__UnibindWasmAbortSignal,
            event: &str,
            listener: &::js_sys::Function,
        );
        #[wasm_bindgen(method, structural, js_name = "removeEventListener")]
        fn remove_event_listener(
            this: &__UnibindWasmAbortSignal,
            event: &str,
            listener: &::js_sys::Function,
        );
    }
    /// The rejection an aborted call settles with, on the same channel as
    /// every other generated error.
    fn __unibind_wasm_aborted() -> ::wasm_bindgen::JsValue {
        __unibind_wasm_error(::std::string::String::from("__unibind__:aborted"))
    }
    /// Race `future` against `signal`; the shared body of every async
    /// export. The `biased` arm keeps an abort that raced completion
    /// deterministic (the abort wins), and dropping the user future is the
    /// cancellation.
    async fn __unibind_wasm_with_abort<__UnibindOutput>(
        signal: ::std::option::Option<::js_sys::Object>,
        future: impl ::std::future::Future<Output = __UnibindOutput>,
    ) -> ::std::result::Result<__UnibindOutput, ::wasm_bindgen::JsValue> {
        let ::std::option::Option::Some(signal) = signal else {
            return ::std::result::Result::Ok(future.await);
        };
        let signal = ::wasm_bindgen::JsCast::unchecked_into::<
            __UnibindWasmAbortSignal,
        >(signal);
        if signal.aborted() {
            return ::std::result::Result::Err(__unibind_wasm_aborted());
        }
        let notify = ::std::sync::Arc::new(::tokio::sync::Notify::new());
        let notifier = ::std::sync::Arc::clone(&notify);
        let listener = ::wasm_bindgen::closure::Closure::<
            dyn ::std::ops::FnMut(),
        >::new(move || notifier.notify_one());
        let callback = ::wasm_bindgen::JsCast::unchecked_ref::<
            ::js_sys::Function,
        >(::std::convert::AsRef::<::wasm_bindgen::JsValue>::as_ref(&listener));
        signal.add_event_listener("abort", callback);
        let settled = ::tokio::select! {
            biased; () = notify.notified() => {
            ::std::result::Result::Err(__unibind_wasm_aborted()) } value = future =>
            ::std::result::Result::Ok(value),
        };
        signal.remove_event_listener("abort", callback);
        settled
    }
    /// A conversion's reason string as the `JsValue` a wrapper rejects
    /// with. Every refusal the glue raises passes through here, so the
    /// error channel has exactly one spelling.
    #[allow(dead_code)]
    fn __unibind_wasm_error(reason: ::std::string::String) -> ::wasm_bindgen::JsValue {
        ::wasm_bindgen::JsValue::from(::js_sys::Error::new(&reason))
    }
    /// One structured argument, out of the `JsValue` JavaScript sent.
    #[allow(dead_code)]
    fn __unibind_wasm_from_js<__UnibindValue>(
        value: ::wasm_bindgen::JsValue,
    ) -> ::std::result::Result<__UnibindValue, ::std::string::String>
    where
        __UnibindValue: ::serde::de::DeserializeOwned,
    {
        ::serde_wasm_bindgen::from_value(value)
            .map_err(|error| ::std::string::ToString::to_string(&error))
    }
    /// One structured value, into the `JsValue` JavaScript receives.
    ///
    /// `json_compatible` picks the shape the ts backend's napi records
    /// already have: a map is a plain object rather than an ES `Map`, and
    /// an absent value is `null`. Two backends, one wire vocabulary.
    #[allow(dead_code)]
    fn __unibind_wasm_to_js<__UnibindValue>(
        value: &__UnibindValue,
    ) -> ::std::result::Result<::wasm_bindgen::JsValue, ::std::string::String>
    where
        __UnibindValue: ::serde::Serialize + ?Sized,
    {
        let serializer = ::serde_wasm_bindgen::Serializer::json_compatible();
        ::serde::Serialize::serialize(value, &serializer)
            .map_err(|error| ::std::string::ToString::to_string(&error))
    }
    /// A path as the JavaScript string a signature declares. Refuses a
    /// path that is not valid UTF-8, the same verdict serde reaches for a
    /// path inside a record.
    #[allow(dead_code)]
    fn __unibind_wasm_path_to_string(
        path: ::std::path::PathBuf,
    ) -> ::std::result::Result<::std::string::String, ::std::string::String> {
        path.into_os_string()
            .into_string()
            .map_err(|_| {
                ::std::string::String::from(
                    "a path that is not valid UTF-8 cannot cross to JavaScript",
                )
            })
    }
    /// A `number` JavaScript sent that the declared Rust width cannot
    /// hold exactly: fractional, non-finite, negative where unsigned,
    /// or outside the double-exact +/-(2^53 - 1) range. Deliberately
    /// not a `__unibind__:` reason: this is a caller mistake, not a
    /// boundary failure the user's error enum declared.
    #[allow(dead_code)]
    fn __unibind_wasm_int_out_of_range(
        width: &str,
        value: f64,
    ) -> ::std::string::String {
        ::std::format!("{} is not a safe integer for a Rust `{}`", value, width)
    }
    /// Narrow a JavaScript `number` to `i64`, refusing a value that is not a safe integer in the width instead of truncating it.
    #[allow(dead_code)]
    fn __unibind_wasm_number_to_i64(
        value: f64,
    ) -> ::std::result::Result<i64, ::std::string::String> {
        const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
        if !value.is_finite() || value.fract() != 0.0 || value.abs() > MAX_SAFE_INTEGER {
            return ::std::result::Result::Err(
                __unibind_wasm_int_out_of_range("i64", value),
            );
        }
        ::std::result::Result::Ok(value as i64)
    }
    #[derive(::serde::Serialize, ::serde::Deserialize)]
    pub struct __UnibindWasmRecordRow {
        #[serde(rename = "id")]
        pub id: f64,
        #[serde(rename = "rowLabel")]
        pub name: ::std::string::String,
        #[serde(rename = "tags")]
        pub tags: ::std::vec::Vec<::std::string::String>,
        #[serde(rename = "weights")]
        pub weights: ::std::collections::HashMap<::std::string::String, f64>,
        #[serde(rename = "blob")]
        pub blob: ::std::vec::Vec<u8>,
        #[serde(rename = "home")]
        #[serde(default)]
        pub home: ::std::option::Option<::std::path::PathBuf>,
    }
    #[allow(dead_code)]
    impl __UnibindWasmRecordRow {
        /// The record as JavaScript sees it.
        fn __unibind_from(value: __unibind_user::Row) -> Self {
            Self {
                id: value.id as f64,
                name: value.name,
                tags: value.tags,
                weights: value.weights,
                blob: value.blob,
                home: value.home,
            }
        }
        /// The record as the user's code takes it; a `number` outside a
        /// field's declared width, or a word outside a field enum's set, is
        /// refused here rather than coerced.
        fn __unibind_into(
            self,
        ) -> ::std::result::Result<__unibind_user::Row, ::std::string::String> {
            ::std::result::Result::Ok(__unibind_user::Row {
                id: __unibind_wasm_number_to_i64(self.id)?,
                name: self.name,
                tags: self.tags,
                weights: self.weights,
                blob: self.blob,
                home: self.home,
            })
        }
    }
    /// Map `SampleError` onto a decodable rejection message, text from `Display`.
    #[allow(dead_code)]
    fn __unibind_wasm_err_SampleError(
        error: __unibind_user::SampleError,
    ) -> ::wasm_bindgen::JsValue {
        let message = ::std::string::ToString::to_string(&error);
        match error {
            __unibind_user::SampleError::StoreGone { .. } => {
                __unibind_wasm_error(
                    ::std::format!(
                        "{}{}", "__unibind__:err:SampleError:StoreGone:", message
                    ),
                )
            }
            __unibind_user::SampleError::Invalid { .. } => {
                __unibind_wasm_error(
                    ::std::format!(
                        "{}{}", "__unibind__:err:SampleError:Invalid:", message
                    ),
                )
            }
        }
    }
    ///Fetch rows.
    ///
    ///Docs reach the generated `.d.ts`.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "rows")]
    pub fn rows(
        store: ::std::string::String,
        limit: ::std::option::Option<u32>,
        root: ::std::option::Option<::std::string::String>,
    ) -> ::std::result::Result<::wasm_bindgen::JsValue, ::wasm_bindgen::JsValue> {
        match __unibind_user::rows(
            store.as_str(),
            limit.unwrap_or(10),
            root.as_deref(),
        ) {
            ::std::result::Result::Ok(value) => {
                __unibind_wasm_to_js::<
                    ::std::vec::Vec<__UnibindWasmRecordRow>,
                >(
                        &value
                            .into_iter()
                            .map(|__unibind_element| __UnibindWasmRecordRow::__unibind_from(
                                __unibind_element,
                            ))
                            .collect::<::std::vec::Vec<_>>(),
                    )
                    .map_err(__unibind_wasm_error)
            }
            ::std::result::Result::Err(error) => {
                ::std::result::Result::Err(__unibind_wasm_err_SampleError(error))
            }
        }
    }
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "touchPath")]
    pub fn touch(
        path: ::std::string::String,
        data: ::std::vec::Vec<u8>,
        ratio: ::std::option::Option<f64>,
        note: ::std::option::Option<::std::string::String>,
    ) -> bool {
        let value = __unibind_user::touch(
            ::std::path::Path::new(path.as_str()),
            data.as_slice(),
            ratio.unwrap_or(0.5),
            note.unwrap_or_else(|| ::std::string::String::from("note")).as_str(),
        );
        value
    }
    ///Wrapping byte sum; a sync export occupies the engine's only thread, so
    ///there is nothing for `blocking` to free and the wasm backend refuses it.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "checksum")]
    pub fn checksum(data: ::std::vec::Vec<u8>) -> u32 {
        let value = __unibind_user::checksum(data.as_slice());
        value
    }
    ///Add, slowly.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "slowAdd")]
    pub fn slow_add(
        a: f64,
        b: f64,
        __unibind_signal: ::std::option::Option<::js_sys::Object>,
    ) -> ::js_sys::Promise {
        ::wasm_bindgen_futures::future_to_promise(async move {
            let a = __unibind_wasm_number_to_i64(a).map_err(__unibind_wasm_error)?;
            let b = __unibind_wasm_number_to_i64(b).map_err(__unibind_wasm_error)?;
            let value = __unibind_wasm_with_abort(
                    __unibind_signal,
                    __unibind_user::slow_add(a, b),
                )
                .await?;
            ::std::result::Result::Ok(::wasm_bindgen::JsValue::from(value as f64))
        })
    }
    ///Fetch one row.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "fetch")]
    pub fn fetch(
        store: ::std::string::String,
        __unibind_signal: ::std::option::Option<::js_sys::Object>,
    ) -> ::js_sys::Promise {
        ::wasm_bindgen_futures::future_to_promise(async move {
            match __unibind_wasm_with_abort(
                    __unibind_signal,
                    __unibind_user::fetch(store),
                )
                .await?
            {
                ::std::result::Result::Ok(value) => {
                    __unibind_wasm_to_js::<
                        __UnibindWasmRecordRow,
                    >(&__UnibindWasmRecordRow::__unibind_from(value))
                        .map_err(__unibind_wasm_error)
                }
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(__unibind_wasm_err_SampleError(error))
                }
            }
        })
    }
    ///Tail rows as a pull stream.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "tail")]
    pub fn tail(store: ::std::string::String) -> __UnibindWasmStreamTail {
        let value = __unibind_user::tail(store.as_str());
        __UnibindWasmStreamTail::__unibind_from(value)
    }
    ///Tail rows once the store opens (an async stream function).
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "tailLater")]
    pub fn tail_later(
        store: ::std::string::String,
        __unibind_signal: ::std::option::Option<::js_sys::Object>,
    ) -> ::js_sys::Promise {
        ::wasm_bindgen_futures::future_to_promise(async move {
            match __unibind_wasm_with_abort(
                    __unibind_signal,
                    __unibind_user::tail_later(store),
                )
                .await?
            {
                ::std::result::Result::Ok(value) => {
                    ::std::result::Result::Ok(
                        ::wasm_bindgen::JsValue::from(
                            __UnibindWasmStreamTailLater::__unibind_from(value),
                        ),
                    )
                }
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(__unibind_wasm_err_SampleError(error))
                }
            }
        })
    }
    ///Open a counter from a free function (the non-constructor path).
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "openCounter")]
    pub fn open_counter(
        start: f64,
    ) -> ::std::result::Result<__UnibindWasmObjectCounter, ::wasm_bindgen::JsValue> {
        let start = __unibind_wasm_number_to_i64(start).map_err(__unibind_wasm_error)?;
        let value = __unibind_user::open_counter(start);
        ::std::result::Result::Ok(__UnibindWasmObjectCounter::__unibind_from(value))
    }
    ///A counter resource.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "Counter")]
    pub struct __UnibindWasmObjectCounter {
        inner: ::std::sync::Arc<__unibind_user::Counter>,
        closed: ::std::sync::atomic::AtomicBool,
    }
    impl __UnibindWasmObjectCounter {
        fn __unibind_from(value: __unibind_user::Counter) -> Self {
            Self {
                inner: ::std::sync::Arc::new(value),
                closed: ::std::sync::atomic::AtomicBool::new(false),
            }
        }
    }
    #[::wasm_bindgen::prelude::wasm_bindgen(js_class = "Counter")]
    impl __UnibindWasmObjectCounter {
        ///Open a counter.
        #[wasm_bindgen(constructor)]
        pub fn new(
            start: ::std::option::Option<f64>,
        ) -> ::std::result::Result<__UnibindWasmObjectCounter, ::wasm_bindgen::JsValue> {
            let start = match start {
                ::std::option::Option::Some(start) => {
                    __unibind_wasm_number_to_i64(start).map_err(__unibind_wasm_error)?
                }
                ::std::option::Option::None => 0,
            };
            match __unibind_user::Counter::new(start) {
                ::std::result::Result::Ok(value) => {
                    ::std::result::Result::Ok(
                        __UnibindWasmObjectCounter::__unibind_from(value),
                    )
                }
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(__unibind_wasm_err_SampleError(error))
                }
            }
        }
        ///Current value.
        #[wasm_bindgen(js_name = "value")]
        pub fn value(&self) -> f64 {
            let value = self.inner.value();
            value as f64
        }
        ///Add and return the new value.
        #[wasm_bindgen(js_name = "addSlowly")]
        pub fn add(
            &self,
            amount: f64,
            __unibind_signal: ::std::option::Option<::js_sys::Object>,
        ) -> ::js_sys::Promise {
            let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
            ::wasm_bindgen_futures::future_to_promise(async move {
                let amount = __unibind_wasm_number_to_i64(amount)
                    .map_err(__unibind_wasm_error)?;
                match __unibind_wasm_with_abort(
                        __unibind_signal,
                        __unibind_inner.add(amount),
                    )
                    .await?
                {
                    ::std::result::Result::Ok(value) => {
                        ::std::result::Result::Ok(
                            ::wasm_bindgen::JsValue::from(value as f64),
                        )
                    }
                    ::std::result::Result::Err(error) => {
                        ::std::result::Result::Err(__unibind_wasm_err_SampleError(error))
                    }
                }
            })
        }
        ///Every value the counter takes, as a pull stream.
        #[wasm_bindgen(js_name = "watch")]
        pub fn watch(&self) -> __UnibindWasmStreamCounterWatch {
            let value = self.inner.watch();
            __UnibindWasmStreamCounterWatch::__unibind_from(value)
        }
        ///Labels under `prefix` (an async, throwing, renamed stream method,
        ///whose handle class is scoped by its owner).
        #[wasm_bindgen(js_name = "tailRows")]
        pub fn tail(
            &self,
            prefix: ::std::string::String,
            limit: ::std::option::Option<u32>,
            __unibind_signal: ::std::option::Option<::js_sys::Object>,
        ) -> ::js_sys::Promise {
            let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
            ::wasm_bindgen_futures::future_to_promise(async move {
                match __unibind_wasm_with_abort(
                        __unibind_signal,
                        __unibind_inner.tail(prefix, limit.unwrap_or(10)),
                    )
                    .await?
                {
                    ::std::result::Result::Ok(value) => {
                        ::std::result::Result::Ok(
                            ::wasm_bindgen::JsValue::from(
                                __UnibindWasmStreamCounterTail::__unibind_from(value),
                            ),
                        )
                    }
                    ::std::result::Result::Err(error) => {
                        ::std::result::Result::Err(__unibind_wasm_err_SampleError(error))
                    }
                }
            })
        }
        ///Fork a counter: a method handing back another object handle.
        #[wasm_bindgen(js_name = "fork")]
        pub fn fork(&self) -> __UnibindWasmObjectCounter {
            let value = self.inner.fork();
            __UnibindWasmObjectCounter::__unibind_from(value)
        }
        ///Release the counter.
        #[wasm_bindgen(js_name = "close")]
        pub fn close(&self) -> ::js_sys::Promise {
            let __unibind_first = !self
                .closed
                .swap(true, ::std::sync::atomic::Ordering::SeqCst);
            let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
            ::wasm_bindgen_futures::future_to_promise(async move {
                if __unibind_first {
                    __unibind_inner.close().await;
                }
                ::std::result::Result::Ok(::wasm_bindgen::JsValue::UNDEFINED)
            })
        }
    }
    /// Pull handle over the stream returned by `tail`.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "TailStream")]
    pub struct __UnibindWasmStreamTail {
        stream: ::std::sync::Arc<::unibind_runtime::PullStream<__unibind_user::Row>>,
    }
    impl __UnibindWasmStreamTail {
        fn __unibind_from(
            stream: ::unibind_runtime::UniStream<__unibind_user::Row>,
        ) -> Self {
            Self {
                stream: ::std::sync::Arc::new(::unibind_runtime::PullStream::new(stream)),
            }
        }
    }
    #[::wasm_bindgen::prelude::wasm_bindgen(js_class = "TailStream")]
    impl __UnibindWasmStreamTail {
        /// The next element, or `null` once the stream ends or closes.
        #[wasm_bindgen(js_name = "next")]
        pub fn next(&self) -> ::js_sys::Promise {
            let __unibind_stream = ::std::sync::Arc::clone(&self.stream);
            ::wasm_bindgen_futures::future_to_promise(async move {
                match __unibind_stream.next().await {
                    ::std::option::Option::Some(value) => {
                        __unibind_wasm_to_js::<
                            __UnibindWasmRecordRow,
                        >(&__UnibindWasmRecordRow::__unibind_from(value))
                            .map_err(__unibind_wasm_error)
                    }
                    ::std::option::Option::None => {
                        ::std::result::Result::Ok(::wasm_bindgen::JsValue::NULL)
                    }
                }
            })
        }
        /// Drop the stream early; a pull in flight resolves `null`, and the
        /// producer sees its stream dropped.
        #[wasm_bindgen(js_name = "close")]
        pub fn close(&self) {
            self.stream.close();
        }
    }
    /// Pull handle over the stream returned by `tail_later`.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "TailLaterStream")]
    pub struct __UnibindWasmStreamTailLater {
        stream: ::std::sync::Arc<::unibind_runtime::PullStream<__unibind_user::Row>>,
    }
    impl __UnibindWasmStreamTailLater {
        fn __unibind_from(
            stream: ::unibind_runtime::UniStream<__unibind_user::Row>,
        ) -> Self {
            Self {
                stream: ::std::sync::Arc::new(::unibind_runtime::PullStream::new(stream)),
            }
        }
    }
    #[::wasm_bindgen::prelude::wasm_bindgen(js_class = "TailLaterStream")]
    impl __UnibindWasmStreamTailLater {
        /// The next element, or `null` once the stream ends or closes.
        #[wasm_bindgen(js_name = "next")]
        pub fn next(&self) -> ::js_sys::Promise {
            let __unibind_stream = ::std::sync::Arc::clone(&self.stream);
            ::wasm_bindgen_futures::future_to_promise(async move {
                match __unibind_stream.next().await {
                    ::std::option::Option::Some(value) => {
                        __unibind_wasm_to_js::<
                            __UnibindWasmRecordRow,
                        >(&__UnibindWasmRecordRow::__unibind_from(value))
                            .map_err(__unibind_wasm_error)
                    }
                    ::std::option::Option::None => {
                        ::std::result::Result::Ok(::wasm_bindgen::JsValue::NULL)
                    }
                }
            })
        }
        /// Drop the stream early; a pull in flight resolves `null`, and the
        /// producer sees its stream dropped.
        #[wasm_bindgen(js_name = "close")]
        pub fn close(&self) {
            self.stream.close();
        }
    }
    /// Pull handle over the stream returned by `Counter.watch`.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "CounterWatchStream")]
    pub struct __UnibindWasmStreamCounterWatch {
        stream: ::std::sync::Arc<::unibind_runtime::PullStream<i64>>,
    }
    impl __UnibindWasmStreamCounterWatch {
        fn __unibind_from(stream: ::unibind_runtime::UniStream<i64>) -> Self {
            Self {
                stream: ::std::sync::Arc::new(::unibind_runtime::PullStream::new(stream)),
            }
        }
    }
    #[::wasm_bindgen::prelude::wasm_bindgen(js_class = "CounterWatchStream")]
    impl __UnibindWasmStreamCounterWatch {
        /// The next element, or `null` once the stream ends or closes.
        #[wasm_bindgen(js_name = "next")]
        pub fn next(&self) -> ::js_sys::Promise {
            let __unibind_stream = ::std::sync::Arc::clone(&self.stream);
            ::wasm_bindgen_futures::future_to_promise(async move {
                match __unibind_stream.next().await {
                    ::std::option::Option::Some(value) => {
                        ::std::result::Result::Ok(
                            ::wasm_bindgen::JsValue::from(value as f64),
                        )
                    }
                    ::std::option::Option::None => {
                        ::std::result::Result::Ok(::wasm_bindgen::JsValue::NULL)
                    }
                }
            })
        }
        /// Drop the stream early; a pull in flight resolves `null`, and the
        /// producer sees its stream dropped.
        #[wasm_bindgen(js_name = "close")]
        pub fn close(&self) {
            self.stream.close();
        }
    }
    /// Pull handle over the stream returned by `Counter.tail`.
    #[::wasm_bindgen::prelude::wasm_bindgen(js_name = "CounterTailRowsStream")]
    pub struct __UnibindWasmStreamCounterTail {
        stream: ::std::sync::Arc<::unibind_runtime::PullStream<::std::string::String>>,
    }
    impl __UnibindWasmStreamCounterTail {
        fn __unibind_from(
            stream: ::unibind_runtime::UniStream<::std::string::String>,
        ) -> Self {
            Self {
                stream: ::std::sync::Arc::new(::unibind_runtime::PullStream::new(stream)),
            }
        }
    }
    #[::wasm_bindgen::prelude::wasm_bindgen(js_class = "CounterTailRowsStream")]
    impl __UnibindWasmStreamCounterTail {
        /// The next element, or `null` once the stream ends or closes.
        #[wasm_bindgen(js_name = "next")]
        pub fn next(&self) -> ::js_sys::Promise {
            let __unibind_stream = ::std::sync::Arc::clone(&self.stream);
            ::wasm_bindgen_futures::future_to_promise(async move {
                match __unibind_stream.next().await {
                    ::std::option::Option::Some(value) => {
                        ::std::result::Result::Ok(::wasm_bindgen::JsValue::from(value))
                    }
                    ::std::option::Option::None => {
                        ::std::result::Result::Ok(::wasm_bindgen::JsValue::NULL)
                    }
                }
            })
        }
        /// Drop the stream early; a pull in flight resolves `null`, and the
        /// producer sees its stream dropped.
        #[wasm_bindgen(js_name = "close")]
        pub fn close(&self) {
            self.stream.close();
        }
    }
}

