#[doc(hidden)]
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    dead_code,
    unused_qualifications
)]
mod __unibind_swift_sample {
    #[swift_bridge::bridge]
    mod __unibind_ffi {
        enum __UnibindSampleError {
            StoreGone(String),
            Invalid(String),
        }
        extern "Rust" {
            type __UnibindRow;
            fn __unibind_new_row(
                id: u64,
                name: String,
                tags: __UnibindVecOfString,
                weights: __UnibindMapOfStringToF64,
                blob: Vec<u8>,
                home: Option<String>,
            ) -> __UnibindRow;
            fn id(self: &__UnibindRow) -> u64;
            fn name(self: &__UnibindRow) -> String;
            fn tags(self: &__UnibindRow) -> __UnibindVecOfString;
            fn weights(self: &__UnibindRow) -> __UnibindMapOfStringToF64;
            fn blob(self: &__UnibindRow) -> Vec<u8>;
            fn home(self: &__UnibindRow) -> Option<String>;
            type __UnibindMapOfStringToF64;
            fn __unibind_new_map_of_string_to_f64() -> __UnibindMapOfStringToF64;
            fn insert(self: &mut __UnibindMapOfStringToF64, key: String, value: f64);
            fn len(self: &__UnibindMapOfStringToF64) -> usize;
            fn key_at(self: &__UnibindMapOfStringToF64, index: usize) -> String;
            fn value_at(self: &__UnibindMapOfStringToF64, index: usize) -> f64;
            type __UnibindMapOfStringToVecOfF64;
            fn __unibind_new_map_of_string_to_vec_of_f64() -> __UnibindMapOfStringToVecOfF64;
            fn insert(
                self: &mut __UnibindMapOfStringToVecOfF64,
                key: String,
                value: Vec<f64>,
            );
            fn len(self: &__UnibindMapOfStringToVecOfF64) -> usize;
            fn key_at(self: &__UnibindMapOfStringToVecOfF64, index: usize) -> String;
            fn value_at(self: &__UnibindMapOfStringToVecOfF64, index: usize) -> Vec<f64>;
            type __UnibindOptionOfRow;
            fn __unibind_new_option_of_row_some(
                value: __UnibindRow,
            ) -> __UnibindOptionOfRow;
            fn __unibind_new_option_of_row_none() -> __UnibindOptionOfRow;
            fn is_some(self: &__UnibindOptionOfRow) -> bool;
            fn value(self: &__UnibindOptionOfRow) -> __UnibindRow;
            type __UnibindVecOfRow;
            fn __unibind_new_vec_of_row() -> __UnibindVecOfRow;
            fn push(self: &mut __UnibindVecOfRow, value: __UnibindRow);
            fn len(self: &__UnibindVecOfRow) -> usize;
            fn get(self: &__UnibindVecOfRow, index: usize) -> __UnibindRow;
            type __UnibindVecOfString;
            fn __unibind_new_vec_of_string() -> __UnibindVecOfString;
            fn push(self: &mut __UnibindVecOfString, value: String);
            fn len(self: &__UnibindVecOfString) -> usize;
            fn get(self: &__UnibindVecOfString, index: usize) -> String;
            fn __unibind_fn_rows(
                store: String,
                limit: usize,
                root: Option<String>,
            ) -> Result<__UnibindVecOfRow, __UnibindSampleError>;
            fn __unibind_fn_weights_echo(
                weights: __UnibindMapOfStringToF64,
            ) -> __UnibindMapOfStringToF64;
            fn __unibind_fn_touch(
                path: String,
                data: Vec<u8>,
                ratio: f64,
                note: String,
                flush: bool,
            ) -> bool;
            fn __unibind_fn_first(rows: __UnibindVecOfRow) -> __UnibindOptionOfRow;
            fn __unibind_fn_echo_option_string(value: Option<String>) -> Option<String>;
            fn __unibind_fn_series(
                table: __UnibindMapOfStringToVecOfF64,
            ) -> __UnibindMapOfStringToVecOfF64;
            fn __unibind_fn_echo_isize(value: isize) -> isize;
            fn __unibind_fn_count(rows: __UnibindVecOfRow) -> usize;
            fn __unibind_fn_echo_bytes(value: Vec<u8>) -> Vec<u8>;
            fn __unibind_fn_echo_f32(value: f32) -> f32;
        }
    }
    pub struct __UnibindRow(super::sample::Row);
    impl __UnibindRow {
        fn from_value(value: super::sample::Row) -> Self {
            Self(value)
        }
        fn into_value(self) -> super::sample::Row {
            self.0
        }
        fn id(&self) -> u64 {
            self.0.id.clone()
        }
        fn name(&self) -> String {
            self.0.name.clone()
        }
        fn tags(&self) -> __UnibindVecOfString {
            __UnibindVecOfString::from_value(self.0.tags.clone())
        }
        fn weights(&self) -> __UnibindMapOfStringToF64 {
            __UnibindMapOfStringToF64::from_value(self.0.weights.clone())
        }
        fn blob(&self) -> Vec<u8> {
            self.0.blob.clone()
        }
        fn home(&self) -> Option<String> {
            self.0.home.clone().map(|value| value.to_string_lossy().into_owned())
        }
    }
    fn __unibind_new_row(
        id: u64,
        name: String,
        tags: __UnibindVecOfString,
        weights: __UnibindMapOfStringToF64,
        blob: Vec<u8>,
        home: Option<String>,
    ) -> __UnibindRow {
        __UnibindRow(super::sample::Row {
            id: id,
            name: name,
            tags: tags.into_value(),
            weights: weights.into_value(),
            blob: blob,
            home: home.map(::std::path::PathBuf::from),
        })
    }
    pub struct __UnibindMapOfStringToF64(::std::vec::Vec<(::std::string::String, f64)>);
    impl __UnibindMapOfStringToF64 {
        /// Entries sorted by key, so Swift-side iteration is
        /// deterministic (`HashMap` order is randomized per process).
        fn from_value(
            value: ::std::collections::HashMap<::std::string::String, f64>,
        ) -> Self {
            let mut entries: ::std::vec::Vec<(::std::string::String, f64)> = value
                .into_iter()
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Self(entries)
        }
        fn into_value(self) -> ::std::collections::HashMap<::std::string::String, f64> {
            self.0.into_iter().collect()
        }
        fn insert(&mut self, key: String, value: f64) {
            self.0.push((key, value));
        }
        fn len(&self) -> usize {
            self.0.len()
        }
        fn key_at(&self, index: usize) -> String {
            self.0[index].0.clone()
        }
        fn value_at(&self, index: usize) -> f64 {
            self.0[index].1.clone()
        }
    }
    fn __unibind_new_map_of_string_to_f64() -> __UnibindMapOfStringToF64 {
        __UnibindMapOfStringToF64(::std::vec::Vec::new())
    }
    pub struct __UnibindMapOfStringToVecOfF64(
        ::std::vec::Vec<(::std::string::String, ::std::vec::Vec<f64>)>,
    );
    impl __UnibindMapOfStringToVecOfF64 {
        /// Entries sorted by key, so Swift-side iteration is
        /// deterministic (`HashMap` order is randomized per process).
        fn from_value(
            value: ::std::collections::HashMap<
                ::std::string::String,
                ::std::vec::Vec<f64>,
            >,
        ) -> Self {
            let mut entries: ::std::vec::Vec<
                (::std::string::String, ::std::vec::Vec<f64>),
            > = value.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Self(entries)
        }
        fn into_value(
            self,
        ) -> ::std::collections::HashMap<::std::string::String, ::std::vec::Vec<f64>> {
            self.0.into_iter().collect()
        }
        fn insert(&mut self, key: String, value: Vec<f64>) {
            self.0.push((key, value));
        }
        fn len(&self) -> usize {
            self.0.len()
        }
        fn key_at(&self, index: usize) -> String {
            self.0[index].0.clone()
        }
        fn value_at(&self, index: usize) -> Vec<f64> {
            self.0[index].1.clone()
        }
    }
    fn __unibind_new_map_of_string_to_vec_of_f64() -> __UnibindMapOfStringToVecOfF64 {
        __UnibindMapOfStringToVecOfF64(::std::vec::Vec::new())
    }
    pub struct __UnibindOptionOfRow(::std::option::Option<super::sample::Row>);
    impl __UnibindOptionOfRow {
        fn from_value(value: ::std::option::Option<super::sample::Row>) -> Self {
            Self(value)
        }
        fn into_value(self) -> ::std::option::Option<super::sample::Row> {
            self.0
        }
        fn is_some(&self) -> bool {
            self.0.is_some()
        }
        /// The payload; the overlay only calls this behind `is_some`.
        fn value(&self) -> __UnibindRow {
            __UnibindRow::from_value(
                self.0.clone().expect("unibind option read while none"),
            )
        }
    }
    fn __unibind_new_option_of_row_some(value: __UnibindRow) -> __UnibindOptionOfRow {
        __UnibindOptionOfRow(::std::option::Option::Some(value.into_value()))
    }
    fn __unibind_new_option_of_row_none() -> __UnibindOptionOfRow {
        __UnibindOptionOfRow(::std::option::Option::None)
    }
    pub struct __UnibindVecOfRow(::std::vec::Vec<super::sample::Row>);
    impl __UnibindVecOfRow {
        fn from_value(value: ::std::vec::Vec<super::sample::Row>) -> Self {
            Self(value)
        }
        fn into_value(self) -> ::std::vec::Vec<super::sample::Row> {
            self.0
        }
        fn push(&mut self, value: __UnibindRow) {
            self.0.push(value.into_value());
        }
        fn len(&self) -> usize {
            self.0.len()
        }
        fn get(&self, index: usize) -> __UnibindRow {
            __UnibindRow::from_value(self.0[index].clone())
        }
    }
    fn __unibind_new_vec_of_row() -> __UnibindVecOfRow {
        __UnibindVecOfRow(::std::vec::Vec::new())
    }
    pub struct __UnibindVecOfString(::std::vec::Vec<::std::string::String>);
    impl __UnibindVecOfString {
        fn from_value(value: ::std::vec::Vec<::std::string::String>) -> Self {
            Self(value)
        }
        fn into_value(self) -> ::std::vec::Vec<::std::string::String> {
            self.0
        }
        fn push(&mut self, value: String) {
            self.0.push(value);
        }
        fn len(&self) -> usize {
            self.0.len()
        }
        fn get(&self, index: usize) -> String {
            self.0[index].clone()
        }
    }
    fn __unibind_new_vec_of_string() -> __UnibindVecOfString {
        __UnibindVecOfString(::std::vec::Vec::new())
    }
    impl ::std::convert::From<super::sample::SampleError>
    for __unibind_ffi::__UnibindSampleError {
        fn from(error: super::sample::SampleError) -> Self {
            let message = ::std::string::ToString::to_string(&error);
            match error {
                super::sample::SampleError::StoreGone { .. } => Self::StoreGone(message),
                super::sample::SampleError::Invalid { .. } => Self::Invalid(message),
            }
        }
    }
    fn __unibind_fn_rows(
        store: String,
        limit: usize,
        root: Option<String>,
    ) -> ::std::result::Result<__UnibindVecOfRow, __unibind_ffi::__UnibindSampleError> {
        let root = root.map(::std::path::PathBuf::from);
        match super::sample::rows(store.as_str(), limit, root.as_deref()) {
            ::std::result::Result::Ok(value) => {
                ::std::result::Result::Ok(__UnibindVecOfRow::from_value(value))
            }
            ::std::result::Result::Err(error) => {
                ::std::result::Result::Err(::std::convert::From::from(error))
            }
        }
    }
    fn __unibind_fn_weights_echo(
        weights: __UnibindMapOfStringToF64,
    ) -> __UnibindMapOfStringToF64 {
        __UnibindMapOfStringToF64::from_value(
            super::sample::weights_echo(weights.into_value()),
        )
    }
    fn __unibind_fn_touch(
        path: String,
        data: Vec<u8>,
        ratio: f64,
        note: String,
        flush: bool,
    ) -> bool {
        let path = ::std::path::PathBuf::from(path);
        super::sample::touch(
            path.as_path(),
            data.as_slice(),
            ratio,
            note.as_str(),
            flush,
        )
    }
    fn __unibind_fn_first(rows: __UnibindVecOfRow) -> __UnibindOptionOfRow {
        __UnibindOptionOfRow::from_value(super::sample::first(rows.into_value()))
    }
    fn __unibind_fn_echo_option_string(value: Option<String>) -> Option<String> {
        super::sample::echo_option_string(value)
    }
    fn __unibind_fn_series(
        table: __UnibindMapOfStringToVecOfF64,
    ) -> __UnibindMapOfStringToVecOfF64 {
        __UnibindMapOfStringToVecOfF64::from_value(
            super::sample::series(table.into_value()),
        )
    }
    fn __unibind_fn_echo_isize(value: isize) -> isize {
        super::sample::echo_isize(value)
    }
    fn __unibind_fn_count(rows: __UnibindVecOfRow) -> usize {
        super::sample::count(rows.into_value())
    }
    fn __unibind_fn_echo_bytes(value: Vec<u8>) -> Vec<u8> {
        super::sample::echo_bytes(value)
    }
    fn __unibind_fn_echo_f32(value: f32) -> f32 {
        super::sample::echo_f32(value)
    }
}

