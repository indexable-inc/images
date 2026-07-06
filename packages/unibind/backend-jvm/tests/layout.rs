//! Prove the computed `#[repr(C)]` layout model against the real compiler.
//!
//! The generators bake these numbers into both sides of the boundary, so
//! the layout algorithm itself is defended here with `size_of`/`align_of`/
//! `offset_of` over hand-written mirror structs shaped like the generated
//! ones.

// The mirror structs exist only to be measured; their fields are never read.
#![allow(dead_code)]

use core::mem::{align_of, offset_of, size_of};

use unibind_backend_jvm::ctype::CTy;
use unibind_backend_jvm::model::Model;
use unibind_core::ir;

#[repr(C)]
struct CString {
    ptr: *mut u8,
    len: usize,
}

#[repr(C)]
struct CVec<T> {
    ptr: *mut T,
    len: usize,
}

#[repr(C)]
struct COption<T> {
    present: u8,
    value: T,
}

#[repr(C)]
struct CPair<K, V> {
    key: K,
    value: V,
}

/// The mirror the generator emits for the sample fixture's `Row`.
#[repr(C)]
struct RowC {
    id: u64,
    name: CString,
    tags: CVec<CString>,
    weights: CVec<CPair<CString, f64>>,
    blob: CString,
    home: COption<CString>,
}

/// The envelope the generator emits for the sample fixture's `rows`.
#[repr(C)]
struct RowsEnvelope {
    code: i32,
    err_msg: CString,
    value: CVec<RowC>,
}

fn model_interface() -> ir::Interface {
    serde_json::from_str(include_str!("snapshots/sample.ir.json")).expect("IR snapshot parses")
}

fn assert_layout<T>(model: &Model<'_>, ty: &CTy) {
    let layout = model.layout(ty);
    assert_eq!(layout.size, size_of::<T>() as u64, "size of {}", ty.mangle());
    assert_eq!(layout.align, align_of::<T>() as u64, "align of {}", ty.mangle());
}

#[test]
fn layouts_match_the_compiler() {
    let interface = model_interface();
    let model = Model::new(&interface).expect("sample interface validates");

    assert_layout::<CString>(&model, &CTy::Str);
    assert_layout::<CString>(&model, &CTy::Path);
    assert_layout::<CString>(&model, &CTy::Bytes);
    assert_layout::<CVec<CString>>(&model, &CTy::Vec(Box::new(CTy::Str)));
    assert_layout::<COption<u8>>(&model, &CTy::Option(Box::new(CTy::Bool)));
    assert_layout::<COption<i32>>(&model, &CTy::Option(Box::new(CTy::Int(ir::IntKind::I32))));
    assert_layout::<COption<CString>>(&model, &CTy::Option(Box::new(CTy::Str)));
    assert_layout::<RowC>(&model, &CTy::Record("Row".to_owned()));

    assert_eq!(
        model.option_value_offset(&CTy::Str),
        offset_of!(COption<CString>, value) as u64
    );
    assert_eq!(
        model.option_value_offset(&CTy::Bool),
        offset_of!(COption<u8>, value) as u64
    );

    let pair = model.pair_struct(&CTy::Str, &CTy::Float(ir::FloatKind::F64));
    assert_eq!(pair.layout.size, size_of::<CPair<CString, f64>>() as u64);
    assert_eq!(pair.offsets[0], offset_of!(CPair<CString, f64>, key) as u64);
    assert_eq!(pair.offsets[1], offset_of!(CPair<CString, f64>, value) as u64);

    let row = model.record_struct("Row");
    let expected = [
        offset_of!(RowC, id),
        offset_of!(RowC, name),
        offset_of!(RowC, tags),
        offset_of!(RowC, weights),
        offset_of!(RowC, blob),
        offset_of!(RowC, home),
    ];
    let actual: Vec<u64> = row.offsets;
    let expected: Vec<u64> = expected.iter().map(|offset| *offset as u64).collect();
    assert_eq!(actual, expected);
}

#[test]
fn envelopes_match_the_compiler() {
    let interface = model_interface();
    let model = Model::new(&interface).expect("sample interface validates");

    let ret = CTy::Vec(Box::new(CTy::Record("Row".to_owned())));
    let envelope = model.envelope(Some(&ret));
    assert_eq!(envelope.layout.size, size_of::<RowsEnvelope>() as u64);
    assert_eq!(envelope.layout.align, align_of::<RowsEnvelope>() as u64);
    assert_eq!(envelope.err_msg_offset, offset_of!(RowsEnvelope, err_msg) as u64);
    assert_eq!(envelope.value_offset, Some(offset_of!(RowsEnvelope, value) as u64));

    let unit = model.envelope(None);
    assert_eq!(unit.value_offset, None);
    assert_eq!(unit.err_msg_offset, 8);
    assert_eq!(unit.layout.size, 24);
}
