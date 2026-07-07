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
        fn __unibind_new_option_of_row_some(value: __UnibindRow) -> __UnibindOptionOfRow;
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

