//! An ignore glob must lower `duplication_pct`: the CLI recomputes stats over
//! the surviving (gated) code after filtering, so ignoring a duplicated file
//! removes both its duplicated lines and its lines from the denominator.

use std::{path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

/// A function body large enough to clear the default clone thresholds.
fn func(name: &str) -> String {
    format!(
        "fn {name}(input: i64) -> i64 {{\n    let mut total = 0;\n    for step in 0..input {{\n        total += step * 2;\n        total -= 1;\n    }}\n    total + 42\n}}\n"
    )
}

fn duplication_pct(dir: &Path, extra_args: &[&str]) -> f64 {
    let mut args = vec!["--pretty", "."];
    args.extend_from_slice(extra_args);
    let output = Command::new(env!("CARGO_BIN_EXE_clone"))
        .current_dir(dir)
        .args(&args)
        .output()
        .expect("clone binary should run");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {stdout}"));
    json["stats"]["duplication_pct"]
        .as_f64()
        .unwrap_or_else(|| panic!("no duplication_pct in {json:#}"))
}

#[test]
fn ignore_glob_lowers_duplication_pct() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path();

    // Three files: a.rs and b.rs are structural duplicates (a clone), c.rs is a
    // duplicate pair too but lives under a `generated/` subtree we will ignore.
    std::fs::write(path.join("a.rs"), func("alpha")).unwrap();
    std::fs::write(path.join("b.rs"), func("beta")).unwrap();
    std::fs::create_dir(path.join("generated")).unwrap();
    std::fs::write(path.join("generated").join("g1.rs"), func("gen_one")).unwrap();
    std::fs::write(path.join("generated").join("g2.rs"), func("gen_two")).unwrap();

    // A permissive global budget so the run exits 0 and we can read the number.
    std::fs::write(
        path.join("clone.toml"),
        "min_lines = 3\nmin_nodes = 5\n[budget]\nglobal_pct = 100.0\n",
    )
    .unwrap();

    let without_ignore = duplication_pct(path, &[]);
    // Ignoring the generated subtree drops those duplicated lines AND their
    // lines from the denominator, so the metric must strictly decrease.
    let with_ignore = duplication_pct(path, &["--ignore", "*/generated/*"]);

    assert!(
        without_ignore > 0.0,
        "baseline should detect duplication: {without_ignore}"
    );
    assert!(
        with_ignore < without_ignore,
        "ignoring a duplicated subtree must lower duplication_pct: \
         without={without_ignore} with={with_ignore}"
    );
}
