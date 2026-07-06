//! Snapshot the Python host files for a representative interface, and unit
//! test the artifact byte-parsing seam. The committed snapshots are the
//! review surface for what `unibind-gen py` writes; on drift the test prints
//! the new content to copy over the snapshot file. The interface is built
//! literally (every IR field is `pub`) so the fixture exercises renames,
//! docs, defaults, and every boundary type without lowering Rust source.

use unibind_core::ir;
use unibind_gen::artifact::parse_ir_bytes;
use unibind_gen::host::HostEmitter as _;
use unibind_gen::py::PyEmitter;

fn names(py: Option<&str>) -> ir::Names {
    ir::Names {
        py: py.map(str::to_owned),
    }
}

fn docs(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

fn arg(name: &str, ty: ir::Type, default: Option<ir::Literal>) -> ir::Arg {
    ir::Arg {
        name: name.to_owned(),
        names: names(None),
        ty,
        default,
    }
}

fn field(name: &str, py: Option<&str>, doc_lines: &[&str], ty: ir::Type) -> ir::Field {
    ir::Field {
        name: name.to_owned(),
        names: names(py),
        docs: docs(doc_lines),
        ty,
    }
}

const fn owned_string() -> ir::Type {
    ir::Type::String { owned: true }
}

fn function(name: &str, py: Option<&str>, doc_lines: &[&str], args: Vec<ir::Arg>) -> ir::Function {
    ir::Function {
        name: name.to_owned(),
        names: names(py),
        docs: docs(doc_lines),
        asyncness: ir::Asyncness::Sync,
        args,
        ret: None,
        throws: None,
    }
}

fn sample_functions() -> Vec<ir::Function> {
    let rows = ir::Function {
        ret: Some(ir::Type::Vec(Box::new(ir::Type::Named("Row".to_owned())))),
        throws: Some("SampleError".to_owned()),
        ..function(
            "rows",
            None,
            &["Fetch rows.", "", "Docs become docstrings."],
            vec![
                arg("store", ir::Type::String { owned: false }, None),
                arg("limit", ir::Type::Int(ir::IntKind::Usize), Some(ir::Literal::Int(10))),
            ],
        )
    };
    let write = function(
        "write",
        Some("write_file"),
        &["Write `data` to `path`."],
        vec![
            arg("path", ir::Type::Path { owned: false }, None),
            arg("data", ir::Type::Bytes { owned: false }, None),
            arg("overwrite", ir::Type::Bool, Some(ir::Literal::Bool(false))),
        ],
    );
    let find = ir::Function {
        ret: Some(ir::Type::Map {
            key: Box::new(owned_string()),
            value: Box::new(ir::Type::Named("Row".to_owned())),
        }),
        throws: Some("SampleError".to_owned()),
        ..function(
            "find",
            None,
            &[],
            vec![
                arg("pattern", owned_string(), None),
                arg("root", ir::Type::Option(Box::new(ir::Type::Path { owned: false })), None),
            ],
        )
    };
    let greet = ir::Function {
        ret: Some(owned_string()),
        ..function(
            "greet",
            None,
            &[],
            vec![
                arg("name", owned_string(), Some(ir::Literal::Str("hello \"world\"\n".to_owned()))),
                arg("ratio", ir::Type::Float(ir::FloatKind::F64), Some(ir::Literal::Float(1.0))),
                arg("note", ir::Type::Option(Box::new(owned_string())), Some(ir::Literal::None)),
            ],
        )
    };
    vec![rows, write, find, greet]
}

fn sample_records() -> Vec<ir::Record> {
    vec![
        ir::Record {
            name: "Row".to_owned(),
            names: names(None),
            docs: docs(&["One result row."]),
            fields: vec![
                field("id", None, &["Identifier."], ir::Type::Int(ir::IntKind::U64)),
                field("name", Some("label"), &[], owned_string()),
                field("tags", None, &[], ir::Type::Vec(Box::new(owned_string()))),
                field(
                    "scores",
                    None,
                    &[],
                    ir::Type::Map {
                        key: Box::new(owned_string()),
                        value: Box::new(ir::Type::Float(ir::FloatKind::F64)),
                    },
                ),
            ],
        },
        ir::Record {
            name: "Source".to_owned(),
            names: names(None),
            docs: docs(&[]),
            fields: vec![field(
                "path",
                None,
                &["Where the row came from."],
                ir::Type::Path { owned: true },
            )],
        },
    ]
}

fn sample_errors() -> Vec<ir::ErrorType> {
    vec![ir::ErrorType {
        name: "SampleError".to_owned(),
        names: names(None),
        docs: docs(&["Everything the sample boundary raises."]),
        py_base: Some("ValueError".to_owned()),
        variants: vec![
            ir::ErrorVariant {
                name: "Parse".to_owned(),
                names: names(Some("ParseError")),
                docs: docs(&["The input did not parse."]),
            },
            ir::ErrorVariant {
                name: "Io".to_owned(),
                names: names(None),
                docs: docs(&[]),
            },
        ],
    }]
}

fn interface() -> ir::Interface {
    ir::Interface {
        version: ir::IR_VERSION,
        name: "_sample".to_owned(),
        names: names(None),
        docs: docs(&[
            "A sample boundary for the emitter tests.",
            "",
            "Everything the phase 1 generator renders appears here once.",
        ]),
        functions: sample_functions(),
        records: sample_records(),
        enums: vec![],
        errors: sample_errors(),
        objects: vec![],
    }
}

fn assert_snapshot(actual: &str, expected: &str, name: &str) {
    if actual.trim() == expected.trim() {
        return;
    }
    println!("=== actual {name} ===");
    println!("{actual}");
    println!("=== end {name} ===");
    panic!("{name} drifted; copy the printed block into tests/snapshots/{name}");
}

#[test]
fn py_host_files_snapshot() {
    let emitter = PyEmitter {
        package: "sample".to_owned(),
        skip_init: false,
    };
    let files = emitter.emit(&interface()).expect("emits");
    let by_path = |path: &str| {
        files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("no emitted file at {path}"))
    };

    assert_snapshot(
        &by_path("sample/_sample.pyi").contents,
        include_str!("snapshots/sample.pyi"),
        "sample.pyi",
    );
    assert_snapshot(
        &by_path("sample/__init__.py").contents,
        include_str!("snapshots/sample.init.py"),
        "sample.init.py",
    );
    assert!(by_path("sample/py.typed").contents.is_empty());
}

#[test]
fn skip_init_drops_the_wrapper() {
    let emitter = PyEmitter {
        package: "sample".to_owned(),
        skip_init: true,
    };
    let files = emitter.emit(&interface()).expect("emits");
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["sample/_sample.pyi", "sample/py.typed"]);
}

#[test]
fn parse_tolerates_linker_nul_padding() {
    let json = serde_json::to_vec(&interface()).expect("serializes");
    let mut bytes = vec![0_u8; 8];
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&[0, 0, 0]);
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&[0; 5]);

    let interfaces = parse_ir_bytes(&bytes).expect("parses");
    let parsed_names: Vec<&str> = interfaces
        .iter()
        .map(|interface| interface.name.as_str())
        .collect();
    assert_eq!(parsed_names, ["_sample", "_sample"]);
}

#[test]
fn parse_rejects_a_newer_ir_version() {
    let mut newer = interface();
    newer.version = 99;
    let json = serde_json::to_vec(&newer).expect("serializes");

    let error = parse_ir_bytes(&json).expect_err("version 99 must not parse");
    let message = format!("{error:#}");
    assert!(message.contains("version 99"), "missing embedded version: {message}");
    assert!(
        message.contains(&format!("version {}", ir::IR_VERSION)),
        "missing supported version: {message}"
    );
}
