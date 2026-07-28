pub fn encode(value: u64) -> String {
    let mut buffer = itoa::Buffer::new();
    format!("alpha:{}", buffer.format(value))
}

pub fn encode_float(value: f64) -> String {
    let mut buffer = ryu::Buffer::new();
    format!("alpha:{}", buffer.format(value))
}

/// Panics, so the crate bakes a `file!()` location into its rlib.
///
/// The reference gate in tests/default.nix asserts the linked binary does not
/// retain this crate's source; that assertion is only meaningful while there is
/// a location here for it to retain.
#[must_use]
pub fn checked(value: u64) -> u64 {
    assert!(value > 0, "value must be positive");
    value
}
