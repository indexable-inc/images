///unibind JVM glue for `sample`: `extern "C"` exports consumed by the generated Java Panama binding.
#[doc(hidden)]
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    dead_code,
    missing_docs,
    unsafe_code,
    unused_qualifications
)]
mod __unibind_jvm_sample {
    /// UTF-8 text as pointer + length. Argument values are Java-owned
    /// and only viewed; returned values leak a boxed slice that the
    /// envelope's `Drop` reclaims.
    #[repr(C)]
    pub struct CString {
        pub ptr: *mut u8,
        pub len: usize,
    }
    impl ::core::ops::Drop for CString {
        fn drop(&mut self) {
            if self.ptr.is_null() {
                return;
            }
            drop(unsafe {
                ::std::boxed::Box::from_raw(
                    ::core::ptr::slice_from_raw_parts_mut(self.ptr, self.len),
                )
            });
        }
    }
    /// Raw bytes cross exactly like text.
    pub type CBytes = CString;
    /// Paths cross as UTF-8 text.
    pub type CPath = CString;
    /// A boxed slice as pointer + length.
    #[repr(C)]
    pub struct CVec<T> {
        pub ptr: *mut T,
        pub len: usize,
    }
    impl<T> ::core::ops::Drop for CVec<T> {
        fn drop(&mut self) {
            if self.ptr.is_null() {
                return;
            }
            drop(unsafe {
                ::std::boxed::Box::from_raw(
                    ::core::ptr::slice_from_raw_parts_mut(self.ptr, self.len),
                )
            });
        }
    }
    /// Inline optional: absent means `present == 0` with `value` all
    /// zeroed.
    #[repr(C)]
    pub struct COption<T> {
        pub present: u8,
        pub value: T,
    }
    /// One map entry; a map crosses as `CVec<CPair<K, V>>`.
    #[repr(C)]
    pub struct CPair<K, V> {
        pub key: K,
        pub value: V,
    }
    ///C mirror of `Row`, fields in declaration order.
    #[repr(C)]
    pub struct RowC {
        pub id: u64,
        pub name: CString,
        pub tags: CVec<CString>,
        pub weights: CVec<CPair<CString, f64>>,
        pub blob: CBytes,
        pub home: COption<CPath>,
    }
    ///Return envelope for `rows`: `code` 0 ok, N the N-th `throws` variant, -1 panic.
    #[repr(C)]
    pub struct RowsEnvelope {
        pub code: i32,
        pub err_msg: CString,
        pub value: CVec<RowC>,
    }
    ///Return envelope for `touch`: `code` 0 ok, N the N-th `throws` variant, -1 panic.
    #[repr(C)]
    pub struct TouchEnvelope {
        pub code: i32,
        pub err_msg: CString,
        pub value: u8,
    }
    const _: () = {
        assert!(::core::mem::size_of:: < CBytes > () == 16);
        assert!(::core::mem::align_of:: < CBytes > () == 8);
        assert!(::core::mem::offset_of!(CBytes, ptr) == 0);
        assert!(::core::mem::offset_of!(CBytes, len) == 8);
        assert!(::core::mem::size_of:: < CVec < RowC > > () == 16);
        assert!(::core::mem::align_of:: < CVec < RowC > > () == 8);
        assert!(::core::mem::offset_of!(CVec < RowC >, ptr) == 0);
        assert!(::core::mem::offset_of!(CVec < RowC >, len) == 8);
        assert!(::core::mem::size_of:: < CVec < CString > > () == 16);
        assert!(::core::mem::align_of:: < CVec < CString > > () == 8);
        assert!(::core::mem::offset_of!(CVec < CString >, ptr) == 0);
        assert!(::core::mem::offset_of!(CVec < CString >, len) == 8);
        assert!(::core::mem::size_of:: < CVec < CPair < CString, f64 >> > () == 16);
        assert!(::core::mem::align_of:: < CVec < CPair < CString, f64 >> > () == 8);
        assert!(::core::mem::offset_of!(CVec < CPair < CString, f64 >>, ptr) == 0);
        assert!(::core::mem::offset_of!(CVec < CPair < CString, f64 >>, len) == 8);
        assert!(::core::mem::size_of:: < CPair < CString, f64 > > () == 24);
        assert!(::core::mem::align_of:: < CPair < CString, f64 > > () == 8);
        assert!(::core::mem::offset_of!(CPair < CString, f64 >, key) == 0);
        assert!(::core::mem::offset_of!(CPair < CString, f64 >, value) == 16);
        assert!(::core::mem::size_of:: < COption < CPath > > () == 24);
        assert!(::core::mem::align_of:: < COption < CPath > > () == 8);
        assert!(::core::mem::offset_of!(COption < CPath >, present) == 0);
        assert!(::core::mem::offset_of!(COption < CPath >, value) == 8);
        assert!(::core::mem::size_of:: < COption < CString > > () == 24);
        assert!(::core::mem::align_of:: < COption < CString > > () == 8);
        assert!(::core::mem::offset_of!(COption < CString >, present) == 0);
        assert!(::core::mem::offset_of!(COption < CString >, value) == 8);
        assert!(::core::mem::size_of:: < CPath > () == 16);
        assert!(::core::mem::align_of:: < CPath > () == 8);
        assert!(::core::mem::offset_of!(CPath, ptr) == 0);
        assert!(::core::mem::offset_of!(CPath, len) == 8);
        assert!(::core::mem::size_of:: < RowC > () == 96);
        assert!(::core::mem::align_of:: < RowC > () == 8);
        assert!(::core::mem::offset_of!(RowC, id) == 0);
        assert!(::core::mem::offset_of!(RowC, name) == 8);
        assert!(::core::mem::offset_of!(RowC, tags) == 24);
        assert!(::core::mem::offset_of!(RowC, weights) == 40);
        assert!(::core::mem::offset_of!(RowC, blob) == 56);
        assert!(::core::mem::offset_of!(RowC, home) == 72);
        assert!(::core::mem::size_of:: < CString > () == 16);
        assert!(::core::mem::align_of:: < CString > () == 8);
        assert!(::core::mem::offset_of!(CString, ptr) == 0);
        assert!(::core::mem::offset_of!(CString, len) == 8);
        assert!(::core::mem::size_of:: < RowsEnvelope > () == 40);
        assert!(::core::mem::align_of:: < RowsEnvelope > () == 8);
        assert!(::core::mem::offset_of!(RowsEnvelope, code) == 0);
        assert!(::core::mem::offset_of!(RowsEnvelope, err_msg) == 8);
        assert!(::core::mem::offset_of!(RowsEnvelope, value) == 24);
        assert!(::core::mem::size_of:: < TouchEnvelope > () == 32);
        assert!(::core::mem::align_of:: < TouchEnvelope > () == 8);
        assert!(::core::mem::offset_of!(TouchEnvelope, code) == 0);
        assert!(::core::mem::offset_of!(TouchEnvelope, err_msg) == 8);
        assert!(::core::mem::offset_of!(TouchEnvelope, value) == 24);
    };
    /// View a Java-owned buffer; empty buffers may carry a null pointer.
    unsafe fn view<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
        if len == 0 { &[] } else { unsafe { ::core::slice::from_raw_parts(ptr, len) } }
    }
    /// Borrow Java-owned text; panics (caught at the export boundary)
    /// on invalid UTF-8.
    unsafe fn str_value(value: &CString) -> &str {
        ::core::str::from_utf8(unsafe { view(value.ptr, value.len) })
            .expect("unibind: text crossing the boundary is not valid UTF-8")
    }
    /// An absent or empty text value; drops as a no-op.
    fn null_string() -> CString {
        CString {
            ptr: ::core::ptr::null_mut(),
            len: 0,
        }
    }
    /// Leak owned bytes into a mirror; `Drop` reclaims them.
    fn bytes_value(value: ::std::vec::Vec<u8>) -> CBytes {
        let boxed = value.into_boxed_slice();
        let len = boxed.len();
        CBytes {
            ptr: ::std::boxed::Box::into_raw(boxed).cast::<u8>(),
            len,
        }
    }
    /// Leak an owned string into a mirror.
    fn string_value(value: ::std::string::String) -> CString {
        bytes_value(value.into_bytes())
    }
    /// Leak an owned path as UTF-8 text; panics (caught at the export
    /// boundary) on non-UTF-8 paths.
    fn path_value(value: ::std::path::PathBuf) -> CPath {
        let text = value
            .into_os_string()
            .into_string()
            .expect("unibind: path crossing the boundary is not valid UTF-8");
        string_value(text)
    }
    /// Leak an owned vec of mirrors.
    fn vec_value<T>(values: ::std::vec::Vec<T>) -> CVec<T> {
        let boxed = values.into_boxed_slice();
        let len = boxed.len();
        CVec {
            ptr: ::std::boxed::Box::into_raw(boxed).cast::<T>(),
            len,
        }
    }
    /// Best-effort text from a caught panic payload.
    fn panic_text(
        payload: &(dyn ::std::any::Any + ::core::marker::Send),
    ) -> ::std::string::String {
        if let ::std::option::Option::Some(text) = payload.downcast_ref::<&str>() {
            return ::std::borrow::ToOwned::to_owned(*text);
        }
        if let ::std::option::Option::Some(text) = payload
            .downcast_ref::<::std::string::String>()
        {
            return ::std::clone::Clone::clone(text);
        }
        ::std::borrow::ToOwned::to_owned("panic across the unibind boundary")
    }
    ///Fetch rows.
    ///
    ///Docs become docstrings.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn unibind_jvm_sample_rows(
        store: *const CString,
        limit: usize,
        root: *const COption<CString>,
    ) -> *mut RowsEnvelope {
        let outcome = ::std::panic::catch_unwind(
            ::std::panic::AssertUnwindSafe(|| {
                let store = unsafe { &*store };
                let store = unsafe { str_value(store) };
                let root = unsafe { &*root };
                let root = if (root).present != 0 {
                    ::std::option::Option::Some(unsafe { str_value((&(root).value)) })
                } else {
                    ::std::option::Option::None
                };
                match super::sample::rows(store, limit, root) {
                    ::std::result::Result::Ok(value) => {
                        RowsEnvelope {
                            code: 0,
                            err_msg: null_string(),
                            value: vec_value(
                                (value)
                                    .into_iter()
                                    .map(|element| {
                                        let record = element;
                                        RowC {
                                            id: record.id,
                                            name: string_value(record.name),
                                            tags: vec_value(
                                                (record.tags)
                                                    .into_iter()
                                                    .map(|element| string_value(element))
                                                    .collect::<::std::vec::Vec<_>>(),
                                            ),
                                            weights: vec_value(
                                                (record.weights)
                                                    .into_iter()
                                                    .map(|(key, value)| CPair {
                                                        key: string_value(key),
                                                        value: value,
                                                    })
                                                    .collect::<::std::vec::Vec<_>>(),
                                            ),
                                            blob: bytes_value(record.blob),
                                            home: match record.home {
                                                ::std::option::Option::Some(value) => {
                                                    COption {
                                                        present: 1,
                                                        value: path_value(value),
                                                    }
                                                }
                                                ::std::option::Option::None => {
                                                    COption {
                                                        present: 0,
                                                        value: unsafe { ::core::mem::zeroed() },
                                                    }
                                                }
                                            },
                                        }
                                    })
                                    .collect::<::std::vec::Vec<_>>(),
                            ),
                        }
                    }
                    ::std::result::Result::Err(error) => {
                        RowsEnvelope {
                            code: match &error {
                                super::sample::SampleError::StoreGone { .. } => 1,
                                super::sample::SampleError::Invalid { .. } => 2,
                            },
                            err_msg: string_value(
                                ::std::string::ToString::to_string(&error),
                            ),
                            value: unsafe { ::core::mem::zeroed() },
                        }
                    }
                }
            }),
        );
        let envelope = match outcome {
            ::std::result::Result::Ok(envelope) => envelope,
            ::std::result::Result::Err(payload) => {
                RowsEnvelope {
                    code: -1,
                    err_msg: string_value(panic_text(payload.as_ref())),
                    value: unsafe { ::core::mem::zeroed() },
                }
            }
        };
        ::std::boxed::Box::into_raw(::std::boxed::Box::new(envelope))
    }
    /// Reclaim an envelope returned by the paired export. Null is a
    /// no-op; anything else must come from that export, exactly once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn unibind_jvm_sample_rows__free(envelope: *mut RowsEnvelope) {
        if envelope.is_null() {
            return;
        }
        drop(unsafe { ::std::boxed::Box::from_raw(envelope) });
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn unibind_jvm_sample_touch(
        path: *const CPath,
        data: *const CBytes,
        ratio: f64,
        note: *const CString,
        flush: u8,
    ) -> *mut TouchEnvelope {
        let outcome = ::std::panic::catch_unwind(
            ::std::panic::AssertUnwindSafe(|| {
                let path = unsafe { &*path };
                let path = ::std::path::Path::new(unsafe { str_value(path) });
                let data = unsafe { &*data };
                let data = unsafe { view((data).ptr, (data).len) };
                let note = unsafe { &*note };
                let note = unsafe { str_value(note) };
                let flush = flush != 0;
                let value = super::sample::touch(path, data, ratio, note, flush);
                TouchEnvelope {
                    code: 0,
                    err_msg: null_string(),
                    value: u8::from(value),
                }
            }),
        );
        let envelope = match outcome {
            ::std::result::Result::Ok(envelope) => envelope,
            ::std::result::Result::Err(payload) => {
                TouchEnvelope {
                    code: -1,
                    err_msg: string_value(panic_text(payload.as_ref())),
                    value: unsafe { ::core::mem::zeroed() },
                }
            }
        };
        ::std::boxed::Box::into_raw(::std::boxed::Box::new(envelope))
    }
    /// Reclaim an envelope returned by the paired export. Null is a
    /// no-op; anything else must come from that export, exactly once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn unibind_jvm_sample_touch__free(
        envelope: *mut TouchEnvelope,
    ) {
        if envelope.is_null() {
            return;
        }
        drop(unsafe { ::std::boxed::Box::from_raw(envelope) });
    }
    /// ABI revision of these exports; the Java binding checks it at
    /// load.
    #[unsafe(no_mangle)]
    pub extern "C" fn unibind_jvm_sample_abi_version() -> u32 {
        0
    }
}

