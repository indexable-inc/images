// struct Row: # [:: pyo3 :: pyclass (from_py_object)]
//   field id: # [pyo3 (get)]
//   field name: # [pyo3 (get , name = "label")]
//   field tags: # [pyo3 (get)]
//   field weights: # [pyo3 (get)]
//   field blob: # [pyo3 (get)]
//   field home: # [pyo3 (get)]

#[doc(hidden)]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
mod __unibind_py_sample {
    use ::pyo3::types::PyModuleMethods as _;
    ::pyo3::create_exception!(
        sample, SampleError, ::pyo3::exceptions::PyRuntimeError, "Boundary failures."
    );
    ::pyo3::create_exception!(sample, StoreGoneError, SampleError, "The store is gone.");
    ::pyo3::create_exception!(sample, Invalid, SampleError, "Bad input.");
    ///Map `SampleError` onto its exception class, message from `Display`.
    impl ::std::convert::From<super::sample::SampleError> for ::pyo3::PyErr {
        fn from(error: super::sample::SampleError) -> Self {
            let message = ::std::string::ToString::to_string(&error);
            match error {
                super::sample::SampleError::StoreGone { .. } => {
                    StoreGoneError::new_err(message)
                }
                super::sample::SampleError::Invalid { .. } => Invalid::new_err(message),
            }
        }
    }
    #[::pyo3::pymethods]
    impl super::sample::Row {
        ///A row.
        #[new]
        #[pyo3(signature = (id, label, tags, weights, blob, home = None))]
        fn __unibind_new(
            id: u64,
            label: ::std::string::String,
            tags: ::std::vec::Vec<::std::string::String>,
            weights: ::std::collections::HashMap<::std::string::String, f64>,
            blob: ::std::vec::Vec<u8>,
            home: ::std::option::Option<::std::path::PathBuf>,
        ) -> Self {
            Self {
                id: id,
                name: label,
                tags: tags,
                weights: weights,
                blob: blob,
                home: home,
            }
        }
        #[allow(unused_variables)]
        fn __repr__(
            &self,
            py: ::pyo3::Python<'_>,
        ) -> ::pyo3::PyResult<::std::string::String> {
            let mut parts: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
            parts
                .push(
                    ::std::format!(
                        "{}={}", "id", ::pyo3::types::PyStringMethods::to_string_lossy(&
                        ::pyo3::types::PyAnyMethods::repr(::pyo3::Py::bind(&
                        ::pyo3::IntoPyObjectExt::into_py_any(self.id.clone(), py,) ?,
                        py,),) ?,)
                    ),
                );
            parts
                .push(
                    ::std::format!(
                        "{}={}", "label",
                        ::pyo3::types::PyStringMethods::to_string_lossy(&
                        ::pyo3::types::PyAnyMethods::repr(::pyo3::Py::bind(&
                        ::pyo3::IntoPyObjectExt::into_py_any(self.name.clone(), py,) ?,
                        py,),) ?,)
                    ),
                );
            parts
                .push(
                    ::std::format!(
                        "{}={}", "tags",
                        ::pyo3::types::PyStringMethods::to_string_lossy(&
                        ::pyo3::types::PyAnyMethods::repr(::pyo3::Py::bind(&
                        ::pyo3::IntoPyObjectExt::into_py_any(self.tags.clone(), py,) ?,
                        py,),) ?,)
                    ),
                );
            parts
                .push(
                    ::std::format!(
                        "{}={}", "weights",
                        ::pyo3::types::PyStringMethods::to_string_lossy(&
                        ::pyo3::types::PyAnyMethods::repr(::pyo3::Py::bind(&
                        ::pyo3::IntoPyObjectExt::into_py_any(self.weights.clone(), py,)
                        ?, py,),) ?,)
                    ),
                );
            parts
                .push(
                    ::std::format!(
                        "{}={}", "blob",
                        ::pyo3::types::PyStringMethods::to_string_lossy(&
                        ::pyo3::types::PyAnyMethods::repr(::pyo3::Py::bind(&
                        ::pyo3::IntoPyObjectExt::into_py_any(self.blob.clone(), py,) ?,
                        py,),) ?,)
                    ),
                );
            parts
                .push(
                    ::std::format!(
                        "{}={}", "home",
                        ::pyo3::types::PyStringMethods::to_string_lossy(&
                        ::pyo3::types::PyAnyMethods::repr(::pyo3::Py::bind(&
                        ::pyo3::IntoPyObjectExt::into_py_any(self.home.clone(), py,) ?,
                        py,),) ?,)
                    ),
                );
            ::pyo3::PyResult::Ok(::std::format!("{}({})", "Row", parts.join(", ")))
        }
        /// Shallow dict of this record's fields, keyed by their Python
        /// names. Nested records stay objects; call `to_dict` on them to
        /// go deeper.
        #[allow(unused_variables)]
        fn to_dict<'py>(
            &self,
            py: ::pyo3::Python<'py>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyDict>> {
            let dict = ::pyo3::types::PyDict::new(py);
            ::pyo3::types::PyDictMethods::set_item(
                &dict,
                "id",
                ::pyo3::IntoPyObjectExt::into_py_any(self.id.clone(), py)?,
            )?;
            ::pyo3::types::PyDictMethods::set_item(
                &dict,
                "label",
                ::pyo3::IntoPyObjectExt::into_py_any(self.name.clone(), py)?,
            )?;
            ::pyo3::types::PyDictMethods::set_item(
                &dict,
                "tags",
                ::pyo3::IntoPyObjectExt::into_py_any(self.tags.clone(), py)?,
            )?;
            ::pyo3::types::PyDictMethods::set_item(
                &dict,
                "weights",
                ::pyo3::IntoPyObjectExt::into_py_any(self.weights.clone(), py)?,
            )?;
            ::pyo3::types::PyDictMethods::set_item(
                &dict,
                "blob",
                ::pyo3::IntoPyObjectExt::into_py_any(self.blob.clone(), py)?,
            )?;
            ::pyo3::types::PyDictMethods::set_item(
                &dict,
                "home",
                ::pyo3::IntoPyObjectExt::into_py_any(self.home.clone(), py)?,
            )?;
            ::pyo3::PyResult::Ok(dict)
        }
        /// The field names, in declaration order.
        fn keys(&self) -> ::std::vec::Vec<&'static str> {
            ::std::vec!["id", "label", "tags", "weights", "blob", "home"]
        }
        /// The field values, in declaration order.
        #[allow(unused_variables)]
        fn values(
            &self,
            py: ::pyo3::Python<'_>,
        ) -> ::pyo3::PyResult<::std::vec::Vec<::pyo3::Py<::pyo3::PyAny>>> {
            ::pyo3::PyResult::Ok(
                ::std::vec![
                    ::pyo3::IntoPyObjectExt::into_py_any(self.id.clone(), py) ?,
                    ::pyo3::IntoPyObjectExt::into_py_any(self.name.clone(), py) ?,
                    ::pyo3::IntoPyObjectExt::into_py_any(self.tags.clone(), py) ?,
                    ::pyo3::IntoPyObjectExt::into_py_any(self.weights.clone(), py) ?,
                    ::pyo3::IntoPyObjectExt::into_py_any(self.blob.clone(), py) ?,
                    ::pyo3::IntoPyObjectExt::into_py_any(self.home.clone(), py) ?
                ],
            )
        }
        /// `(name, value)` pairs, in declaration order.
        #[allow(unused_variables)]
        fn items(
            &self,
            py: ::pyo3::Python<'_>,
        ) -> ::pyo3::PyResult<
            ::std::vec::Vec<(&'static str, ::pyo3::Py<::pyo3::PyAny>)>,
        > {
            ::pyo3::PyResult::Ok(
                ::std::vec![
                    ("id", ::pyo3::IntoPyObjectExt::into_py_any(self.id.clone(), py) ?,),
                    ("label", ::pyo3::IntoPyObjectExt::into_py_any(self.name.clone(), py)
                    ?,), ("tags", ::pyo3::IntoPyObjectExt::into_py_any(self.tags.clone(),
                    py) ?,), ("weights", ::pyo3::IntoPyObjectExt::into_py_any(self
                    .weights.clone(), py) ?,), ("blob",
                    ::pyo3::IntoPyObjectExt::into_py_any(self.blob.clone(), py) ?,),
                    ("home", ::pyo3::IntoPyObjectExt::into_py_any(self.home.clone(), py)
                    ?,)
                ],
            )
        }
        /// The field named `key`, or `default` (`None` unset) when the
        /// record has no such field.
        #[allow(unused_variables)]
        #[pyo3(signature = (key, default = None))]
        fn get(
            &self,
            py: ::pyo3::Python<'_>,
            key: &str,
            default: ::std::option::Option<::pyo3::Py<::pyo3::PyAny>>,
        ) -> ::pyo3::PyResult<::std::option::Option<::pyo3::Py<::pyo3::PyAny>>> {
            match key {
                "id" => {
                    ::pyo3::PyResult::Ok(
                        ::std::option::Option::Some(
                            ::pyo3::IntoPyObjectExt::into_py_any(self.id.clone(), py)?,
                        ),
                    )
                }
                "label" => {
                    ::pyo3::PyResult::Ok(
                        ::std::option::Option::Some(
                            ::pyo3::IntoPyObjectExt::into_py_any(self.name.clone(), py)?,
                        ),
                    )
                }
                "tags" => {
                    ::pyo3::PyResult::Ok(
                        ::std::option::Option::Some(
                            ::pyo3::IntoPyObjectExt::into_py_any(self.tags.clone(), py)?,
                        ),
                    )
                }
                "weights" => {
                    ::pyo3::PyResult::Ok(
                        ::std::option::Option::Some(
                            ::pyo3::IntoPyObjectExt::into_py_any(
                                self.weights.clone(),
                                py,
                            )?,
                        ),
                    )
                }
                "blob" => {
                    ::pyo3::PyResult::Ok(
                        ::std::option::Option::Some(
                            ::pyo3::IntoPyObjectExt::into_py_any(self.blob.clone(), py)?,
                        ),
                    )
                }
                "home" => {
                    ::pyo3::PyResult::Ok(
                        ::std::option::Option::Some(
                            ::pyo3::IntoPyObjectExt::into_py_any(self.home.clone(), py)?,
                        ),
                    )
                }
                _ => ::pyo3::PyResult::Ok(default),
            }
        }
        #[allow(unused_variables)]
        fn __getitem__(
            &self,
            py: ::pyo3::Python<'_>,
            key: &str,
        ) -> ::pyo3::PyResult<::pyo3::Py<::pyo3::PyAny>> {
            match key {
                "id" => ::pyo3::IntoPyObjectExt::into_py_any(self.id.clone(), py),
                "label" => ::pyo3::IntoPyObjectExt::into_py_any(self.name.clone(), py),
                "tags" => ::pyo3::IntoPyObjectExt::into_py_any(self.tags.clone(), py),
                "weights" => {
                    ::pyo3::IntoPyObjectExt::into_py_any(self.weights.clone(), py)
                }
                "blob" => ::pyo3::IntoPyObjectExt::into_py_any(self.blob.clone(), py),
                "home" => ::pyo3::IntoPyObjectExt::into_py_any(self.home.clone(), py),
                _ => {
                    ::pyo3::PyResult::Err(
                        ::pyo3::exceptions::PyKeyError::new_err(key.to_owned()),
                    )
                }
            }
        }
        fn __contains__(&self, key: &str) -> bool {
            ["id", "label", "tags", "weights", "blob", "home"].contains(&key)
        }
        fn __len__(&self) -> usize {
            ["id", "label", "tags", "weights", "blob", "home"].len()
        }
        /// Iterates the field names, like a dict.
        fn __iter__<'py>(
            &self,
            py: ::pyo3::Python<'py>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyIterator>> {
            let keys = ::pyo3::Bound::into_any(
                ::pyo3::types::PyList::new(
                    py,
                    ["id", "label", "tags", "weights", "blob", "home"],
                )?,
            );
            ::pyo3::types::PyAnyMethods::try_iter(&keys)
        }
    }
    ///Fetch rows.
    ///
    ///Docs become docstrings.
    #[::pyo3::pyfunction]
    #[pyo3(signature = (store, limit = 10, root = None))]
    fn rows(
        store: &str,
        limit: usize,
        root: ::std::option::Option<&str>,
    ) -> ::pyo3::PyResult<::std::vec::Vec<super::sample::Row>> {
        super::sample::rows(store, limit, root).map_err(::pyo3::PyErr::from)
    }
    #[::pyo3::pyfunction]
    #[pyo3(name = "touch_path")]
    #[pyo3(signature = (path, data, ratio = 0.5, note = "note", flush = false))]
    fn touch(
        path: &::std::path::Path,
        data: ::pyo3::buffer::PyBuffer<u8>,
        ratio: f64,
        note: &str,
        flush: bool,
    ) -> ::pyo3::PyResult<bool> {
        if !data.is_c_contiguous() {
            return ::pyo3::PyResult::Err(
                ::pyo3::exceptions::PyBufferError::new_err(
                    "argument `data` must be a C-contiguous buffer (bytes, bytearray, or a contiguous memoryview)",
                ),
            );
        }
        let __unibind_data_ptr = data.buf_ptr().cast::<u8>();
        let __unibind_data_len = data.item_count();
        #[allow(
            unsafe_code,
            reason = "SAFETY: contiguity was checked above, and the shadowed PyBuffer keeps \
                      its Py_buffer view alive for the whole call, which the buffer protocol \
                      contract says pins the exporter's memory unresized and unfreed"
        )]
        let data: &[u8] = unsafe {
            ::std::slice::from_raw_parts(__unibind_data_ptr, __unibind_data_len)
        };
        ::pyo3::PyResult::Ok(super::sample::touch(path, data, ratio, note, flush))
    }
    ///Wait for one row.
    #[::pyo3::pyfunction]
    #[pyo3(signature = (id, timeout_ms = 250))]
    fn fetch_row<'py>(
        py: ::pyo3::Python<'py>,
        id: u64,
        timeout_ms: u64,
    ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
        ::unibind_py_runtime::future_into_py(
            py,
            async move {
                super::sample::fetch_row(id, timeout_ms)
                    .await
                    .map_err(::pyo3::PyErr::from)
            },
        )
    }
    ///Snapshot the head row.
    #[::pyo3::pyfunction]
    #[pyo3(signature = ())]
    fn head<'py>(
        py: ::pyo3::Python<'py>,
    ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
        ::unibind_py_runtime::future_into_py(
            py,
            async move { ::pyo3::PyResult::Ok(super::sample::head().await) },
        )
    }
    ///Checksum data off the GIL.
    #[::pyo3::pyfunction]
    #[pyo3(signature = (data))]
    fn digest(
        py: ::pyo3::Python<'_>,
        data: ::pyo3::buffer::PyBuffer<u8>,
    ) -> ::pyo3::PyResult<::std::vec::Vec<u8>> {
        if !data.is_c_contiguous() {
            return ::pyo3::PyResult::Err(
                ::pyo3::exceptions::PyBufferError::new_err(
                    "argument `data` must be a C-contiguous buffer (bytes, bytearray, or a contiguous memoryview)",
                ),
            );
        }
        let __unibind_data_ptr = data.buf_ptr().cast::<u8>();
        let __unibind_data_len = data.item_count();
        #[allow(
            unsafe_code,
            reason = "SAFETY: contiguity was checked above, and the shadowed PyBuffer keeps \
                      its Py_buffer view alive for the whole call, which the buffer protocol \
                      contract says pins the exporter's memory unresized and unfreed"
        )]
        let data: &[u8] = unsafe {
            ::std::slice::from_raw_parts(__unibind_data_ptr, __unibind_data_len)
        };
        ::pyo3::PyResult::Ok(py.detach(move || super::sample::digest(data)))
    }
    ///Tick forever.
    #[::pyo3::pyfunction]
    #[pyo3(signature = (period_ms))]
    fn ticks(period_ms: u64) -> UnibindStreamTicks {
        UnibindStreamTicks::__unibind_wrap(super::sample::ticks(period_ms))
    }
    ///Follow rows as they land.
    #[::pyo3::pyfunction]
    #[pyo3(signature = (store))]
    fn follow<'py>(
        py: ::pyo3::Python<'py>,
        store: ::std::string::String,
    ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
        ::unibind_py_runtime::future_into_py(
            py,
            async move {
                super::sample::follow(store)
                    .await
                    .map(UnibindStreamFollow::__unibind_wrap)
                    .map_err(::pyo3::PyErr::from)
            },
        )
    }
    ///Open a cursor.
    #[::pyo3::pyfunction]
    #[pyo3(signature = ())]
    fn cursor<'py>(
        py: ::pyo3::Python<'py>,
    ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
        ::unibind_py_runtime::future_into_py(
            py,
            async move {
                ::pyo3::PyResult::Ok(
                    UnibindObjectCursor::__unibind_wrap(super::sample::cursor().await),
                )
            },
        )
    }
    ///A live store handle.
    #[::pyo3::pyclass(name = "Store", frozen)]
    struct UnibindObjectStore {
        inner: ::std::sync::Arc<super::sample::Store>,
        closed: ::std::sync::atomic::AtomicBool,
    }
    impl UnibindObjectStore {
        fn __unibind_wrap(inner: super::sample::Store) -> Self {
            Self {
                inner: ::std::sync::Arc::new(inner),
                closed: ::std::sync::atomic::AtomicBool::new(false),
            }
        }
    }
    #[::pyo3::pymethods]
    impl UnibindObjectStore {
        ///Open a store.
        #[new]
        #[pyo3(signature = (path))]
        fn __unibind_new(path: &str) -> ::pyo3::PyResult<Self> {
            super::sample::Store::open(path)
                .map(Self::__unibind_wrap)
                .map_err(::pyo3::PyErr::from)
        }
        ///Count rows.
        #[pyo3(signature = ())]
        fn len(&self) -> u64 {
            self.inner.len()
        }
        ///Pull one row.
        #[pyo3(signature = (id))]
        fn get<'py>(
            &self,
            py: ::pyo3::Python<'py>,
            id: u64,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            let inner = ::std::sync::Arc::clone(&self.inner);
            ::unibind_py_runtime::future_into_py(
                py,
                async move { inner.get(id).await.map_err(::pyo3::PyErr::from) },
            )
        }
        ///Flush to disk.
        #[pyo3(name = "sync_all")]
        #[pyo3(signature = ())]
        fn sync(&self) -> bool {
            self.inner.sync()
        }
        ///Release the store.
        fn close<'py>(
            &self,
            py: ::pyo3::Python<'py>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            let first = !self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst);
            let inner = ::std::sync::Arc::clone(&self.inner);
            ::unibind_py_runtime::future_into_py(
                py,
                async move {
                    if first {
                        inner.close().await.map_err(::pyo3::PyErr::from)?;
                    }
                    ::pyo3::PyResult::Ok(())
                },
            )
        }
        ///Enter `async with`: resolves to the object itself.
        fn __aenter__<'py>(
            slf: ::pyo3::Bound<'py, Self>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            let py = slf.py();
            let owned: ::pyo3::Py<Self> = slf.unbind();
            ::unibind_py_runtime::future_into_py(
                py,
                async move { ::pyo3::PyResult::Ok(owned) },
            )
        }
        ///Exit `async with`: closes the resource, never suppresses the exception.
        fn __aexit__<'py>(
            &self,
            py: ::pyo3::Python<'py>,
            _exc_type: ::pyo3::Bound<'py, ::pyo3::PyAny>,
            _exc: ::pyo3::Bound<'py, ::pyo3::PyAny>,
            _tb: ::pyo3::Bound<'py, ::pyo3::PyAny>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            let first = !self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst);
            let inner = ::std::sync::Arc::clone(&self.inner);
            ::unibind_py_runtime::future_into_py(
                py,
                async move {
                    if first {
                        inner.close().await.map_err(::pyo3::PyErr::from)?;
                    }
                    ::pyo3::PyResult::Ok(false)
                },
            )
        }
    }
    impl ::std::ops::Drop for UnibindObjectStore {
        fn drop(&mut self) {
            if self.closed.load(::std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let _ = ::pyo3::Python::try_attach(|py| {
                let category = py.get_type::<::pyo3::exceptions::PyResourceWarning>();
                let _ = ::pyo3::PyErr::warn(
                    py,
                    category.as_any(),
                    c"unclosed Store: call close() or use 'async with'",
                    1,
                );
            });
        }
    }
    ///A cursor over rows.
    #[::pyo3::pyclass(name = "Cursor", frozen)]
    struct UnibindObjectCursor {
        inner: ::std::sync::Arc<super::sample::Cursor>,
    }
    impl UnibindObjectCursor {
        fn __unibind_wrap(inner: super::sample::Cursor) -> Self {
            Self {
                inner: ::std::sync::Arc::new(inner),
            }
        }
    }
    #[::pyo3::pymethods]
    impl UnibindObjectCursor {
        ///Step forward.
        #[pyo3(signature = (by))]
        fn advance(&self, by: u64) -> u64 {
            self.inner.advance(by)
        }
    }
    ///Async iterator produced by `ticks`.
    ///
    ///Pull-based: each `__anext__` polls exactly one item, so the producer only runs as fast as the consumer awaits.
    #[::pyo3::pyclass(name = "TicksStream", frozen)]
    struct UnibindStreamTicks {
        stream: ::unibind_py_runtime::SharedStream<u64>,
    }
    impl UnibindStreamTicks {
        fn __unibind_wrap(stream: ::unibind_runtime::UniStream<u64>) -> Self {
            Self {
                stream: ::unibind_py_runtime::SharedStream::new(stream),
            }
        }
    }
    #[::pyo3::pymethods]
    impl UnibindStreamTicks {
        fn __aiter__(slf: ::pyo3::PyRef<'_, Self>) -> ::pyo3::PyRef<'_, Self> {
            slf
        }
        fn __anext__<'py>(
            &self,
            py: ::pyo3::Python<'py>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            self.stream.next_into_py(py)
        }
    }
    ///Async iterator produced by `follow`.
    ///
    ///Pull-based: each `__anext__` polls exactly one item, so the producer only runs as fast as the consumer awaits.
    #[::pyo3::pyclass(name = "FollowStream", frozen)]
    struct UnibindStreamFollow {
        stream: ::unibind_py_runtime::SharedStream<super::sample::Row>,
    }
    impl UnibindStreamFollow {
        fn __unibind_wrap(
            stream: ::unibind_runtime::UniStream<super::sample::Row>,
        ) -> Self {
            Self {
                stream: ::unibind_py_runtime::SharedStream::new(stream),
            }
        }
    }
    #[::pyo3::pymethods]
    impl UnibindStreamFollow {
        fn __aiter__(slf: ::pyo3::PyRef<'_, Self>) -> ::pyo3::PyRef<'_, Self> {
            slf
        }
        fn __anext__<'py>(
            &self,
            py: ::pyo3::Python<'py>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            self.stream.next_into_py(py)
        }
    }
    ///A sample boundary exercising the phase 0-2 surface.
    #[::pyo3::pymodule]
    #[pyo3(name = "sample")]
    fn __unibind_module(
        module: &::pyo3::Bound<'_, ::pyo3::types::PyModule>,
    ) -> ::pyo3::PyResult<()> {
        module.add_function(::pyo3::wrap_pyfunction!(rows, module)?)?;
        module.add_function(::pyo3::wrap_pyfunction!(touch, module)?)?;
        module.add_function(::pyo3::wrap_pyfunction!(fetch_row, module)?)?;
        module.add_function(::pyo3::wrap_pyfunction!(head, module)?)?;
        module.add_function(::pyo3::wrap_pyfunction!(digest, module)?)?;
        module.add_function(::pyo3::wrap_pyfunction!(ticks, module)?)?;
        module.add_function(::pyo3::wrap_pyfunction!(follow, module)?)?;
        module.add_function(::pyo3::wrap_pyfunction!(cursor, module)?)?;
        module.add_class::<super::sample::Row>()?;
        module.add_class::<UnibindObjectStore>()?;
        module.add_class::<UnibindObjectCursor>()?;
        module.add_class::<UnibindStreamTicks>()?;
        module.add_class::<UnibindStreamFollow>()?;
        module.add("SampleError", module.py().get_type::<SampleError>())?;
        module.add("StoreGoneError", module.py().get_type::<StoreGoneError>())?;
        module.add("Invalid", module.py().get_type::<Invalid>())?;
        module.add("__version__", ::std::env!("CARGO_PKG_VERSION"))?;
        ::pyo3::PyResult::Ok(())
    }
}

