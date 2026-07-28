// Fixture for cargoUnit's `cargoConfigRustflags` option. This crate compiles
// only when `--cfg cargo_config_ok` is set, which the fixture declares in
// `.cargo/config.toml` ([build] rustflags). cargoUnit ignores cargo's config
// unless `cargoConfigRustflags = true`, so a build with the option produces the
// binary and a build without it hits the compile_error below. That makes the
// test behavioral rather than a string assertion on rendered argv.
#[cfg(not(cargo_config_ok))]
compile_error!(
    "cargo_config_ok cfg is unset: cargoUnit did not apply .cargo/config.toml rustflags"
);

fn main() {
    println!("cargo-config rustflags applied");
}
