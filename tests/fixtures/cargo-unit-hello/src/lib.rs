/// Return the fixture greeting.
///
/// ```
/// assert_eq!(cargo_unit_hello::greeting(), "hello from cargo-unit");
/// ```
#[must_use]
pub const fn greeting() -> &'static str {
    "hello from cargo-unit"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_greeting() {
        assert_eq!(greeting(), "hello from cargo-unit");
    }

    #[test]
    fn current_dir_is_writable() {
        std::fs::write(".cargo-unit-writable-cwd-check", "ok").unwrap();
    }

    #[test]
    fn package_build_env_reaches_build_script() {
        // build.rs re-exposed CARGO_UNIT_BUILD_ENV (from packageBuildEnv) as this
        // compile-time var. Only enforce when the workspace opts in by setting the
        // expected value at test runtime (via packageTestEnv); otherwise the build
        // script saw nothing and this is a no-op.
        const FROM_BUILD: &str = env!("CARGO_UNIT_HELLO_BUILD_ENV");
        if let Ok(expected) = std::env::var("CARGO_UNIT_BUILD_ENV_EXPECTED") {
            assert_eq!(FROM_BUILD, expected);
        }
    }

    #[test]
    fn package_test_env_and_path_are_available() {
        assert_eq!(
            std::env::var("CARGO_UNIT_FIXTURE_ENV").as_deref(),
            Ok("ok")
        );

        let output = std::process::Command::new("hello")
            .output()
            .expect("hello should be on PATH");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Hello"));
    }
}
