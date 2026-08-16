//! What the user's own record structs gain: nothing.
//!
//! `wasm-bindgen` has no attribute that makes a plain struct cross by value,
//! so there is no `#[napi(object)]` analogue to attach and no field attribute
//! to rename a key with. The record's whole boundary shape lives in the
//! generated serde twin ([`crate::twin`]) instead, which leaves the user's
//! struct exactly as they wrote it -- the same outcome as the ts backend's
//! mirrored records, reached for every record rather than for the ones whose
//! spellings differ.
//!
//! The empty attribute lists still have to be index-aligned with the IR's
//! records and fields: the macro zips them, so a short list would attach one
//! record's attributes to another.

use unibind_core::ir;
use unibind_core::render::{RenderError, RenderedRecord};

use crate::ty;

/// Empty attributes for one record, aligned with its fields.
pub fn record_attrs(record: &ir::Record) -> RenderedRecord {
    RenderedRecord {
        outer: Vec::new(),
        fields: record.fields.iter().map(|_| Vec::new()).collect(),
    }
}

/// Field types must be representable in both directions; check them before the
/// twin spells tokens that would miscompile.
pub fn check_record(record: &ir::Record) -> Result<(), RenderError> {
    for field in &record.fields {
        ty::check(
            &field.ty,
            &format!("field `{}` of record `{}`", field.name, record.name),
        )?;
    }
    Ok(())
}
