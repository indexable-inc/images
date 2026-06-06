fn main() {
    // Links `prebuilt-mid` directly and `prebuilt-lib` only transitively. When
    // the mid unit is injected as a prebuilt rlib, rustc can resolve the leaf
    // crate that rlib references only if the leaf prebuilt was auto-injected
    // from the mid unit's recorded depUnits.
    println!("{} (answer={})", prebuilt_mid::greeting(), prebuilt_mid::answer());
}
