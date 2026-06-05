fn main() {
    // Links against `prebuilt-lib`. When the lib unit is injected as a prebuilt
    // rlib (no source in the consumer's graph), this still resolves and runs.
    println!("{} (answer={})", prebuilt_lib::greeting(), prebuilt_lib::answer());
}
