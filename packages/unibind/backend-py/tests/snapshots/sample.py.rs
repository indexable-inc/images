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
        #[pyo3(signature = (id, label, tags, weights, blob, home))]
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
        data: &[u8],
        ratio: f64,
        note: &str,
        flush: bool,
    ) -> bool {
        super::sample::touch(path, data, ratio, note, flush)
    }
    ///A sample boundary exercising the phase 0 surface.
    #[::pyo3::pymodule]
    #[pyo3(name = "sample")]
    fn __unibind_module(
        module: &::pyo3::Bound<'_, ::pyo3::types::PyModule>,
    ) -> ::pyo3::PyResult<()> {
        module.add_function(::pyo3::wrap_pyfunction!(rows, module)?)?;
        module.add_function(::pyo3::wrap_pyfunction!(touch, module)?)?;
        module.add_class::<super::sample::Row>()?;
        module.add("SampleError", module.py().get_type::<SampleError>())?;
        module.add("StoreGoneError", module.py().get_type::<StoreGoneError>())?;
        module.add("Invalid", module.py().get_type::<Invalid>())?;
        module.add("__version__", ::std::env!("CARGO_PKG_VERSION"))?;
        ::pyo3::PyResult::Ok(())
    }
}
