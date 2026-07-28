/// Marker the consumer prints so the self-test can prove it linked against the
/// real prebuilt rlib (not a stub) and ran the code inside it.
pub fn greeting() -> String {
    format!("prebuilt-lib:{}", answer())
}

/// A value the test asserts on, so a wrong or empty rlib would change output.
pub fn answer() -> u32 {
    42
}
