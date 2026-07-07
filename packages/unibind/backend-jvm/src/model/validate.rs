//! Interface validation: what the JVM backend accepts, with errors that
//! say why a shape cannot cross.
//!
//! Lowering already guarantees most of this (objects never in arguments,
//! streams only in return position, constructors sync); the checks repeat
//! the rules against the serialized IR anyway, so an out-of-process
//! generator fed a hand-built interface fails with a message instead of a
//! panic deeper in the layout code.

use unibind_core::ir;

use crate::RenderError;

use super::Model;

pub(super) fn interface(
    model: &Model<'_>,
    interface: &ir::Interface,
) -> Result<(), RenderError> {
    if let Some(data_enum) = interface.enums.first() {
        return Err(RenderError::new(format!(
            "`{}` is a data enum, which the JVM backend does not render yet",
            data_enum.name
        )));
    }
    for record in &interface.records {
        check_record(model, &record.name, &mut Vec::new())?;
    }
    for function in &interface.functions {
        check_function(model, interface, function, None)?;
    }
    for object in &interface.objects {
        if let Some(ctor) = &object.constructor {
            check_function(model, interface, ctor, Some(&object.name))?;
        }
        for method in &object.methods {
            check_function(model, interface, method, Some(&object.name))?;
        }
    }
    Ok(())
}

fn check_function(
    model: &Model<'_>,
    interface: &ir::Interface,
    function: &ir::Function,
    owner: Option<&str>,
) -> Result<(), RenderError> {
    let site = owner.map_or_else(
        || function.name.clone(),
        |object| format!("{object}.{}", function.name),
    );
    for arg in &function.args {
        check_arg(model, &arg.ty, &site)?;
    }
    if let Some(ret) = &function.ret {
        check_ret(model, ret, &site)?;
    }
    if let Some(throws) = &function.throws
        && !interface.errors.iter().any(|error| error.name == *throws)
    {
        return Err(RenderError::new(format!(
            "`{site}` returns `Result<_, {throws}>`, but `{throws}` is not a \
             #[unibind::error] in this module"
        )));
    }
    Ok(())
}

/// Arguments carry plain data only: streams cross only as return values,
/// and objects only as handles the target language already holds.
fn check_arg(model: &Model<'_>, ty: &ir::Type, site: &str) -> Result<(), RenderError> {
    match ty {
        ir::Type::Stream(_) => Err(RenderError::new(format!(
            "`{site}` takes a `UniStream` argument; streams cross only as return values"
        ))),
        ir::Type::Named(name) if model.is_object(name) => Err(RenderError::new(format!(
            "`{site}` takes the object `{name}` by value; objects never cross as arguments"
        ))),
        ir::Type::Named(name) => check_record(model, name, &mut Vec::new()),
        ir::Type::Option(inner) | ir::Type::Vec(inner) => check_arg(model, inner, site),
        ir::Type::Map { key, value } => {
            check_arg(model, key, site)?;
            check_arg(model, value, site)
        }
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. } => Ok(()),
    }
}

/// Return types additionally allow a top-level stream (whose items must be
/// plain data) and a top-level object (crossing as a handle).
fn check_ret(model: &Model<'_>, ty: &ir::Type, site: &str) -> Result<(), RenderError> {
    match ty {
        ir::Type::Stream(item) => check_item(model, item, site),
        ir::Type::Named(name) if model.is_object(name) => Ok(()),
        _ => check_arg(model, ty, site),
    }
}

/// Stream items cross inside item envelopes, so they must be plain data:
/// no nested streams, no objects.
fn check_item(model: &Model<'_>, ty: &ir::Type, site: &str) -> Result<(), RenderError> {
    match ty {
        ir::Type::Stream(_) => Err(RenderError::new(format!(
            "`{site}` returns a stream of streams; stream items must be plain data"
        ))),
        ir::Type::Named(name) if model.is_object(name) => Err(RenderError::new(format!(
            "`{site}` streams the object `{name}`; stream items must be plain data"
        ))),
        ir::Type::Named(name) => check_record(model, name, &mut Vec::new()),
        ir::Type::Option(inner) | ir::Type::Vec(inner) => check_item(model, inner, site),
        ir::Type::Map { key, value } => {
            check_item(model, key, site)?;
            check_item(model, value, site)
        }
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. } => Ok(()),
    }
}

fn check_record(model: &Model<'_>, name: &str, stack: &mut Vec<String>) -> Result<(), RenderError> {
    if stack.iter().any(|seen| seen == name) {
        return Err(RenderError::new(format!(
            "record `{name}` is part of a reference cycle; recursive records cannot \
             cross the boundary by value"
        )));
    }
    let Some(record) = model.records.get(name) else {
        return Err(RenderError::new(format!(
            "`{name}` is not a #[unibind::record] in this module"
        )));
    };
    stack.push(name.to_owned());
    for field in &record.fields {
        check_field(model, &record.name, &field.ty, stack)?;
    }
    stack.pop();
    Ok(())
}

/// Record fields cross by value inside the record's mirror struct, so they
/// hold plain data only.
fn check_field(
    model: &Model<'_>,
    record: &str,
    ty: &ir::Type,
    stack: &mut Vec<String>,
) -> Result<(), RenderError> {
    match ty {
        ir::Type::Stream(_) => Err(RenderError::new(format!(
            "record `{record}` holds a `UniStream` field; streams cross only as return values"
        ))),
        ir::Type::Named(name) if model.is_object(name) => Err(RenderError::new(format!(
            "record `{record}` holds the object `{name}`; record fields carry plain data only"
        ))),
        ir::Type::Named(name) => check_record(model, name, stack),
        ir::Type::Option(inner) | ir::Type::Vec(inner) => check_field(model, record, inner, stack),
        ir::Type::Map { key, value } => {
            check_field(model, record, key, stack)?;
            check_field(model, record, value, stack)
        }
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. } => Ok(()),
    }
}
