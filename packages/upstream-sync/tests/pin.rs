//! Gate tests for `upstream-sync pin-drift`, whose entire product is an EXIT
//! CODE. The in-process `verdict()` tests cover the classification; these cover
//! what a caller actually sees, which is the thing that was wrong: the first
//! version of this gate exited 0 on any pin it could not verify, so a lost
//! refs/pins ref or a renamed bookmark bought permanent green. Every case below
//! asserts the status, not the wording.
//!
//! gh is stubbed per endpoint. The forge is not what is under test; which
//! answers become a zero exit is.

mod common;

use std::fs;

use common::{Run, run_bin, stub_path, write_stub};

// One healthy fork (pin == tip), one diverged, one waived-diverged, one whose
// bookmark the forge does not have, and one floating-diverged. `compare` answers
// the 4-field TSV the tool reads; `commits/<bookmark>` answers the tip sha.
const GH_STUB: &str = r#"case "$2" in
  repos/fakefork/good/commits/*) echo "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" ;;
  repos/fakefork/good/compare/*) printf 'identical\t0\t0\taaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n' ;;
  repos/fakefork/off/commits/*) echo "cccccccccccccccccccccccccccccccccccccccc" ;;
  repos/fakefork/off/compare/*) printf 'diverged\t72\t54\tdddddddddddddddddddddddddddddddddddddddd\n' ;;
  repos/fakefork/waived/commits/*) echo "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" ;;
  repos/fakefork/waived/compare/*) printf 'diverged\t1\t1\tffffffffffffffffffffffffffffffffffffffff\n' ;;
  repos/fakefork/floating/commits/*) echo "1111111111111111111111111111111111111111" ;;
  repos/fakefork/floating/compare/*) printf 'diverged\t9\t3\t2222222222222222222222222222222222222222\n' ;;
  *) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
esac"#;

// The forge unreachable rather than answering, verbatim gh output for an expired
// token. Fatal for the same reason it is fatal in drift: no information is not
// the answer "no drift".
const GH_STUB_UNREACHABLE: &str = r#"echo "HTTP 401: Bad credentials (https://api.github.com/graphql)" >&2
exit 1"#;

const LOCK: &str = r#"{"nodes":{
  "good-src":{"locked":{"rev":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
  "off-src":{"locked":{"rev":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}},
  "waived-src":{"locked":{"rev":"9999999999999999999999999999999999999999"}},
  "nobookmark-src":{"locked":{"rev":"7777777777777777777777777777777777777777"}},
  "floating-src":{"locked":{"rev":"8888888888888888888888888888888888888888"}}}}"#;

fn entry(name: &str, repo: &str, auto_update: bool, extra: &str) -> String {
    format!(
        r#"{{"name":"{name}","input":"{name}-src","forkRepo":"fakefork/{repo}",
  "bookmark":"ix-patched","upstreamUrl":"https://github.com/fakeorg/{repo}.git",
  "autoUpdate":{auto_update}{extra},"patches":{{}}}}"#
    )
}

fn mapping_of(entries: &[String]) -> String {
    format!("[{}]", entries.join(","))
}

struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new(stub: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let stubs = dir.path().join("stubs");
        fs::create_dir(&stubs).unwrap();
        write_stub(&stubs, "gh", stub);
        fs::write(dir.path().join("flake.lock"), LOCK).unwrap();
        Self { dir }
    }

    fn run(&self, mapping: &str) -> Run {
        let path = self.dir.path().join("mapping.json");
        fs::write(&path, mapping).unwrap();
        let envs = [("PATH", stub_path(&self.dir.path().join("stubs")))];
        run_bin(
            env!("CARGO_BIN_EXE_upstream-sync"),
            &[
                "pin-drift",
                "--mapping",
                &path.display().to_string(),
                "--json",
            ],
            self.dir.path(),
            &envs,
        )
    }
}

fn row_of(run: &Run, name: &str) -> serde_json::Value {
    let rows: serde_json::Value = serde_json::from_str(&run.stdout)
        .unwrap_or_else(|e| panic!("pin-drift --json is not JSON ({e}):\n{}", run.stdout));
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("no row for {name} in {}", run.stdout))
        .clone()
}

/// One case of "this topology exits with this status".
struct Case {
    /// Fork name, which is also its input name (`<name>-src` in the lock).
    name: &'static str,
    /// Stub repo whose canned compare answer produces the topology.
    repo: &'static str,
    auto_update: bool,
    class: &'static str,
    status: i32,
}

// A table rather than one test per row: the bodies were identical apart from the
// expected values, and two copies of an assertion is how they drift apart. The
// cases that assert on MESSAGE content stay separate below, since that is a
// different claim.
#[test]
fn each_topology_maps_to_the_exit_status_it_should() {
    let cases = [
        Case { name: "good", repo: "good", auto_update: false, class: "current", status: 0 },
        // Floating: legitimately off the branch between a cron rebase and the
        // rolling PR merging, so reported and not failed.
        Case { name: "floating", repo: "floating", auto_update: true, class: "diverged", status: 0 },
        Case { name: "off", repo: "off", auto_update: false, class: "diverged", status: 1 },
        // Unverifiable, which must fail rather than pass: the whole point.
        Case { name: "nobookmark", repo: "nobookmark", auto_update: false, class: "unknown", status: 1 },
    ];
    let h = Harness::new(GH_STUB);
    for case in cases {
        let run = h.run(&mapping_of(&[entry(case.name, case.repo, case.auto_update, "")]));
        assert_eq!(
            run.status, case.status,
            "{}: stdout:\n{}\nstderr:\n{}",
            case.name, run.stdout, run.stderr
        );
        let row = row_of(&run, case.name);
        assert_eq!(row["class"], case.class, "{}: {row}", case.name);
        assert_eq!(row["failed"], case.status != 0, "{}: {row}", case.name);
    }
}

#[test]
fn a_diverged_rev_pinned_fork_names_both_shas_and_the_merge_base() {
    let h = Harness::new(GH_STUB);
    let run = h.run(&mapping_of(&[entry("off", "off", false, "")]));
    assert_eq!(run.status, 1, "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
    // All three have to be in the operator's face, since neither a repin nor a
    // bookmark push is obviously the right move.
    for expected in [
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "cccccccccccccccccccccccccccccccccccccccc",
        "dddddddddddddddddddddddddddddddddddddddd",
    ] {
        assert!(
            run.stderr.contains(expected),
            "failure message omits {expected}:\n{}",
            run.stderr
        );
    }
}

#[test]
fn a_waiver_on_the_pinned_rev_exits_zero_and_a_stale_one_does_not() {
    let h = Harness::new(GH_STUB);
    let waiver = |rev: &str| {
        format!(
            r#","pinDivergence":{{"rev":"{rev}","reason":"ENG-11646: someone has to decide"}}"#
        )
    };

    let live = h.run(&mapping_of(&[entry(
        "waived",
        "waived",
        false,
        &waiver("9999999999999999999999999999999999999999"),
    )]));
    assert_eq!(live.status, 0, "stdout:\n{}\nstderr:\n{}", live.stdout, live.stderr);

    // The pin moved, so an acknowledgement made about the old rev says nothing
    // about this one.
    let stale = h.run(&mapping_of(&[entry(
        "waived",
        "waived",
        false,
        &waiver("3333333333333333333333333333333333333333"),
    )]));
    assert_eq!(stale.status, 1, "stdout:\n{}\nstderr:\n{}", stale.stdout, stale.stderr);
    assert!(stale.stderr.contains("expired"), "{}", stale.stderr);
}

// The regression this file exists for. Each of these is a pin nobody can verify,
// and the first version of the gate exited 0 for all three while printing that
// every pin was on its bookmark.
#[test]
fn a_pin_that_cannot_be_verified_fails_rather_than_passing() {
    let h = Harness::new(GH_STUB);

    // Bookmark renamed or deleted: the forge 404s the tip read.
    let run = h.run(&mapping_of(&[entry("nobookmark", "nobookmark", false, "")]));
    assert_eq!(
        run.status, 1,
        "a missing bookmark must not pass:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert_eq!(row_of(&run, "nobookmark")["class"], "unknown");
    assert!(
        run.stderr.contains("could not be checked"),
        "{}",
        run.stderr
    );

    // Input absent from flake.lock: the registry and the lock disagree.
    let run = h.run(&mapping_of(&[entry("unlocked", "good", false, "")]));
    assert_eq!(
        run.status, 1,
        "an input missing from flake.lock must not pass:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(run.stderr.contains("flake.lock"), "{}", run.stderr);
}

// A waiver over a row nobody could evaluate must not be called dead: the advice
// would be derived from no information, and the waiver may well be live.
#[test]
fn an_unverifiable_pin_is_not_reported_as_a_dead_waiver() {
    let h = Harness::new(GH_STUB);
    let run = h.run(&mapping_of(&[entry(
        "nobookmark",
        "nobookmark",
        false,
        r#","pinDivergence":{"rev":"7777777777777777777777777777777777777777","reason":"ENG-11646: live"}"#,
    )]));
    assert_eq!(run.status, 1, "{}\n{}", run.stdout, run.stderr);
    assert!(
        !run.stderr.contains("delete it"),
        "told to delete a waiver that could not be evaluated:\n{}",
        run.stderr
    );
}

#[test]
fn an_unreachable_forge_is_fatal_rather_than_a_pass() {
    let h = Harness::new(GH_STUB_UNREACHABLE);
    let run = h.run(&mapping_of(&[entry("good", "good", false, "")]));
    assert_ne!(
        run.status, 0,
        "an unreachable forge must not read as no drift:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stderr.contains("cannot reach the forge"),
        "{}",
        run.stderr
    );
}
