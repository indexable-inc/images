/// Marker the chain consumer prints so the self-test can prove it linked the
/// prebuilt mid rlib and the leaf rlib that rlib was compiled against.
pub fn greeting() -> String {
    format!("prebuilt-mid:{}", answer())
}

/// One more than the leaf's answer: 43 from source, 100 when the variant mid
/// rlib (leaf answer = 99) and its recorded leaf dep are both injected.
pub fn answer() -> u32 {
    prebuilt_lib::answer() + 1
}
