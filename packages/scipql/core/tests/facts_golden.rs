//! Golden test for SCIP → facts lowering, the part that needs neither
//! rust-analyzer nor Soufflé. `tests/fixtures/two-sockets/index.scip` was
//! produced by `rust-analyzer scip` on the sibling `src/` (a crate with a
//! `net::Socket` and a `mock::Socket`), both committed here so the test is
//! self-contained in this crate's source (the cargo-unit sandbox sees only
//! this crate). The point: the two same-named structs lower to *distinct*
//! monikers, and byte offsets land on the name. (The full index → rename
//! pipeline is covered by the `scipql-e2e` flake check.)

use std::path::Path;

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn lowers_same_named_structs_to_distinct_monikers() {
    let fixture = manifest_dir().join("tests/fixtures/two-sockets");
    let golden = fixture.join("index.scip");
    let index = scipql_core::load_index(&golden).expect("load golden index");
    let facts = scipql_core::facts_from_index(&index, Some(&fixture)).expect("lower facts");

    let defs: Vec<_> = facts
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.role == "definition")
        .collect();

    let net = defs
        .iter()
        .find(|occurrence| occurrence.symbol.ends_with("net/Socket#"))
        .expect("a net::Socket definition");
    let mock = defs
        .iter()
        .find(|occurrence| occurrence.symbol.ends_with("mock/Socket#"))
        .expect("a mock::Socket definition");

    assert_ne!(
        net.symbol, mock.symbol,
        "the two Sockets must lower to distinct monikers"
    );
    assert_eq!(net.path, "src/net.rs");
    assert_eq!(mock.path, "src/mock.rs");

    // The byte offsets must bracket the `Socket` name in the source.
    let net_src = std::fs::read_to_string(fixture.join("src/net.rs")).expect("read net.rs");
    assert_eq!(
        &net_src[net.start..net.end],
        "Socket",
        "net::Socket definition range must cover the struct name"
    );
}

#[test]
fn schema_declares_every_relation_facts_writes() {
    // The schema fed to Soufflé must declare each relation `write_dir` emits, or
    // a query referencing one would fail to compile.
    for relation in ["occurrence", "symbol_info", "document", "relationship"] {
        assert!(
            scipql_core::SCHEMA.contains(&format!(".decl {relation}(")),
            "SCHEMA must declare `{relation}`"
        );
    }
}
