// struct Row: # [:: napi_derive :: napi (object , js_name = "SampleRow")]
//   field id: 
//   field name: # [napi (js_name = "rowLabel")]
//   field tags: 
//   field weights: 
//   field blob: 
//   field home: 

///A sample boundary exercising the ts surface.
#[doc(hidden)]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
mod __unibind_ts_sample_ts {
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
    ///Map `SampleError` onto a decodable napi rejection reason, message from `Display`.
    impl ::std::convert::From<super::sample_ts::SampleError> for ::napi::Error {
        fn from(error: super::sample_ts::SampleError) -> Self {
            let message = ::std::string::ToString::to_string(&error);
            match error {
                super::sample_ts::SampleError::StoreGone { .. } => {
                    ::napi::Error::from_reason(
                        ::std::format!(
                            "{}{}", "__unibind__:err:SampleError:StoreGone:", message
                        ),
                    )
                }
                super::sample_ts::SampleError::Invalid { .. } => {
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
    ) -> ::napi::Result<::std::vec::Vec<super::sample_ts::Row>> {
        match super::sample_ts::rows(
            store.as_str(),
            limit.unwrap_or(10),
            root.as_deref(),
        ) {
            ::std::result::Result::Ok(value) => ::std::result::Result::Ok(value),
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
        let value = super::sample_ts::touch(
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
        let value = super::sample_ts::checksum(data.as_ref());
        value
    }
    ///Add, slowly.
    #[::napi_derive::napi]
    pub async fn slow_add(
        a: i64,
        b: i64,
        __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
    ) -> ::napi::Result<i64> {
        let __unibind_future = super::sample_ts::slow_add(a, b);
        match __unibind_signal {
            ::std::option::Option::Some(__unibind_signal) => {
                if __unibind_signal.already_aborted {
                    return ::std::result::Result::Err(__unibind_aborted());
                }
                ::tokio::select! {
                    biased; () = __unibind_signal.notify.notified() => {
                    ::std::result::Result::Err(__unibind_aborted()) } value =
                    __unibind_future => ::std::result::Result::Ok(value),
                }
            }
            ::std::option::Option::None => {
                let value = __unibind_future.await;
                ::std::result::Result::Ok(value)
            }
        }
    }
    ///Fetch one row.
    #[::napi_derive::napi]
    pub async fn fetch(
        store: ::std::string::String,
        __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
    ) -> ::napi::Result<super::sample_ts::Row> {
        let __unibind_future = super::sample_ts::fetch(store);
        match __unibind_signal {
            ::std::option::Option::Some(__unibind_signal) => {
                if __unibind_signal.already_aborted {
                    return ::std::result::Result::Err(__unibind_aborted());
                }
                ::tokio::select! {
                    biased; () = __unibind_signal.notify.notified() => {
                    ::std::result::Result::Err(__unibind_aborted()) } value =
                    __unibind_future => match value { ::std::result::Result::Ok(value) =>
                    ::std::result::Result::Ok(value), ::std::result::Result::Err(error)
                    => { ::std::result::Result::Err(::napi::Error::from(error)) } },
                }
            }
            ::std::option::Option::None => {
                let value = __unibind_future.await;
                match value {
                    ::std::result::Result::Ok(value) => ::std::result::Result::Ok(value),
                    ::std::result::Result::Err(error) => {
                        ::std::result::Result::Err(::napi::Error::from(error))
                    }
                }
            }
        }
    }
    ///Tail rows as a pull stream.
    #[::napi_derive::napi]
    pub fn tail(store: ::std::string::String) -> __UnibindStreamTail {
        let value = super::sample_ts::tail(store.as_str());
        __UnibindStreamTail::__unibind_from(value)
    }
    ///Pull handle over the stream returned by `tail`.
    #[::napi_derive::napi(js_name = "TailStream")]
    pub struct __UnibindStreamTail {
        stream: ::std::sync::Mutex<
            ::std::option::Option<::unibind_runtime::UniStream<super::sample_ts::Row>>,
        >,
        pull: ::tokio::sync::Mutex<()>,
        closed: ::tokio::sync::watch::Sender<bool>,
    }
    impl __UnibindStreamTail {
        fn __unibind_from(
            stream: ::unibind_runtime::UniStream<super::sample_ts::Row>,
        ) -> Self {
            Self {
                stream: ::std::sync::Mutex::new(::std::option::Option::Some(stream)),
                pull: ::tokio::sync::Mutex::new(()),
                closed: ::tokio::sync::watch::Sender::new(false),
            }
        }
        fn __unibind_slot(
            &self,
        ) -> ::std::sync::MutexGuard<
            '_,
            ::std::option::Option<::unibind_runtime::UniStream<super::sample_ts::Row>>,
        > {
            self.stream.lock().unwrap_or_else(::std::sync::PoisonError::into_inner)
        }
    }
    #[::napi_derive::napi]
    impl __UnibindStreamTail {
        /// The next element, or `null` once the stream ends or closes.
        #[::napi_derive::napi]
        pub async fn next(&self) -> ::std::option::Option<super::sample_ts::Row> {
            let _pull = self.pull.lock().await;
            let mut stream = self.__unibind_slot().take()?;
            let mut closed = self.closed.subscribe();
            let item = ::tokio::select! {
                biased; _ = closed.wait_for(| closed | * closed) =>
                ::std::option::Option::None, item = stream.next() => item,
            };
            if item.is_some() && !*self.closed.borrow() {
                self.__unibind_slot().replace(stream);
            }
            let value = item?;
            ::std::option::Option::Some(value)
        }
        /// Drop the stream early; a pull in flight resolves `null`, and
        /// the producer sees its stream dropped.
        #[::napi_derive::napi]
        pub fn close(&self) {
            let _ = self.closed.send(true);
            self.__unibind_slot().take();
        }
    }
    ///Tail rows once the store opens (an async stream function).
    #[::napi_derive::napi]
    pub async fn tail_later(
        store: ::std::string::String,
        __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
    ) -> ::napi::Result<__UnibindStreamTailLater> {
        let __unibind_future = super::sample_ts::tail_later(store);
        match __unibind_signal {
            ::std::option::Option::Some(__unibind_signal) => {
                if __unibind_signal.already_aborted {
                    return ::std::result::Result::Err(__unibind_aborted());
                }
                ::tokio::select! {
                    biased; () = __unibind_signal.notify.notified() => {
                    ::std::result::Result::Err(__unibind_aborted()) } value =
                    __unibind_future => match value { ::std::result::Result::Ok(value) =>
                    ::std::result::Result::Ok(__UnibindStreamTailLater::__unibind_from(value)),
                    ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(::napi::Error::from(error)) } },
                }
            }
            ::std::option::Option::None => {
                let value = __unibind_future.await;
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
        }
    }
    ///Pull handle over the stream returned by `tail_later`.
    #[::napi_derive::napi(js_name = "TailLaterStream")]
    pub struct __UnibindStreamTailLater {
        stream: ::std::sync::Mutex<
            ::std::option::Option<::unibind_runtime::UniStream<super::sample_ts::Row>>,
        >,
        pull: ::tokio::sync::Mutex<()>,
        closed: ::tokio::sync::watch::Sender<bool>,
    }
    impl __UnibindStreamTailLater {
        fn __unibind_from(
            stream: ::unibind_runtime::UniStream<super::sample_ts::Row>,
        ) -> Self {
            Self {
                stream: ::std::sync::Mutex::new(::std::option::Option::Some(stream)),
                pull: ::tokio::sync::Mutex::new(()),
                closed: ::tokio::sync::watch::Sender::new(false),
            }
        }
        fn __unibind_slot(
            &self,
        ) -> ::std::sync::MutexGuard<
            '_,
            ::std::option::Option<::unibind_runtime::UniStream<super::sample_ts::Row>>,
        > {
            self.stream.lock().unwrap_or_else(::std::sync::PoisonError::into_inner)
        }
    }
    #[::napi_derive::napi]
    impl __UnibindStreamTailLater {
        /// The next element, or `null` once the stream ends or closes.
        #[::napi_derive::napi]
        pub async fn next(&self) -> ::std::option::Option<super::sample_ts::Row> {
            let _pull = self.pull.lock().await;
            let mut stream = self.__unibind_slot().take()?;
            let mut closed = self.closed.subscribe();
            let item = ::tokio::select! {
                biased; _ = closed.wait_for(| closed | * closed) =>
                ::std::option::Option::None, item = stream.next() => item,
            };
            if item.is_some() && !*self.closed.borrow() {
                self.__unibind_slot().replace(stream);
            }
            let value = item?;
            ::std::option::Option::Some(value)
        }
        /// Drop the stream early; a pull in flight resolves `null`, and
        /// the producer sees its stream dropped.
        #[::napi_derive::napi]
        pub fn close(&self) {
            let _ = self.closed.send(true);
            self.__unibind_slot().take();
        }
    }
    ///Open a counter from a free function (the non-constructor path).
    #[::napi_derive::napi]
    pub fn open_counter(start: i64) -> __UnibindObjectCounter {
        let value = super::sample_ts::open_counter(start);
        __UnibindObjectCounter::__unibind_from(value)
    }
    ///A counter resource.
    #[::napi_derive::napi(js_name = "Counter")]
    pub struct __UnibindObjectCounter {
        inner: ::std::sync::Arc<super::sample_ts::Counter>,
        closed: ::std::sync::atomic::AtomicBool,
    }
    impl __UnibindObjectCounter {
        fn __unibind_from(value: super::sample_ts::Counter) -> Self {
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
        pub fn new(start: ::std::option::Option<i64>) -> ::napi::Result<Self> {
            match super::sample_ts::Counter::new(start.unwrap_or(0)) {
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
        pub fn value(&self) -> i64 {
            let value = self.inner.value();
            value
        }
        ///Add and return the new value.
        #[::napi_derive::napi(js_name = "addSlowly")]
        pub async fn add(
            &self,
            amount: i64,
            __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
        ) -> ::napi::Result<i64> {
            let __unibind_future = {
                let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
                async move { __unibind_inner.add(amount).await }
            };
            match __unibind_signal {
                ::std::option::Option::Some(__unibind_signal) => {
                    if __unibind_signal.already_aborted {
                        return ::std::result::Result::Err(__unibind_aborted());
                    }
                    ::tokio::select! {
                        biased; () = __unibind_signal.notify.notified() => {
                        ::std::result::Result::Err(__unibind_aborted()) } value =
                        __unibind_future => match value {
                        ::std::result::Result::Ok(value) =>
                        ::std::result::Result::Ok(value),
                        ::std::result::Result::Err(error) => {
                        ::std::result::Result::Err(::napi::Error::from(error)) } },
                    }
                }
                ::std::option::Option::None => {
                    let value = __unibind_future.await;
                    match value {
                        ::std::result::Result::Ok(value) => {
                            ::std::result::Result::Ok(value)
                        }
                        ::std::result::Result::Err(error) => {
                            ::std::result::Result::Err(::napi::Error::from(error))
                        }
                    }
                }
            }
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
}
