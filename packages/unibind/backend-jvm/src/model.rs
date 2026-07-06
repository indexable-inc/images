//! The validated type model of one interface.
//!
//! [`Model::new`] is the shared gate all three generators pass through: it
//! rejects the surface the sync JVM backend does not implement and resolves
//! every `Named` type, so the layout queries and the per-record lookups can
//! assume a well-formed interface.

use std::collections::BTreeMap;

use unibind_core::ir;

use crate::ctype::{self, CTy, EnvelopeLayout, Layout, StructLayout};
use crate::RenderError;

/// Record lookup plus layout queries for every reachable mirror.
pub struct Model<'ir> {
    records: BTreeMap<&'ir str, &'ir ir::Record>,
}

impl<'ir> Model<'ir> {
    /// Validate the sync JVM surface and build the model.
    ///
    /// # Errors
    ///
    /// Fails for surface this backend does not implement (async functions,
    /// data enums, objects), for unresolved or recursive record types, and
    /// for a `throws` name with no matching error enum.
    pub fn new(interface: &'ir ir::Interface) -> Result<Self, RenderError> {
        if let Some(object) = interface.objects.first() {
            return Err(RenderError::new(format!(
                "`{}` is a #[unibind::object]; objects land in phase 2 (issue #1992)",
                object.name
            )));
        }
        if let Some(data_enum) = interface.enums.first() {
            return Err(RenderError::new(format!(
                "`{}` is a data enum, which the sync JVM backend does not render (issue #2083)",
                data_enum.name
            )));
        }
        if let Some(function) = interface
            .functions
            .iter()
            .find(|function| matches!(function.asyncness, ir::Asyncness::Async))
        {
            return Err(RenderError::new(format!(
                "`{}` is async; async functions land in phase 2 (issue #1992)",
                function.name
            )));
        }
        let model = Self {
            records: interface
                .records
                .iter()
                .map(|record| (record.name.as_str(), record))
                .collect(),
        };
        for record in &interface.records {
            model.check_record(&record.name, &mut Vec::new())?;
        }
        for function in &interface.functions {
            for arg in &function.args {
                model.check_type(&arg.ty, &mut Vec::new())?;
            }
            if let Some(ret) = &function.ret {
                model.check_type(ret, &mut Vec::new())?;
            }
            if let Some(throws) = &function.throws
                && !interface.errors.iter().any(|error| error.name == *throws)
            {
                return Err(RenderError::new(format!(
                    "`{}` returns `Result<_, {throws}>`, but `{throws}` is not a \
                     #[unibind::error] in this module",
                    function.name
                )));
            }
        }
        Ok(model)
    }

    fn check_type(&self, ty: &ir::Type, stack: &mut Vec<String>) -> Result<(), RenderError> {
        match ty {
            ir::Type::Option(inner) | ir::Type::Vec(inner) => self.check_type(inner, stack),
            ir::Type::Map { key, value } => {
                self.check_type(key, stack)?;
                self.check_type(value, stack)
            }
            ir::Type::Named(name) => self.check_record(name, stack),
            ir::Type::Bool
            | ir::Type::Int(_)
            | ir::Type::Float(_)
            | ir::Type::String { .. }
            | ir::Type::Path { .. }
            | ir::Type::Bytes { .. } => Ok(()),
        }
    }

    fn check_record(&self, name: &str, stack: &mut Vec<String>) -> Result<(), RenderError> {
        if stack.iter().any(|seen| seen == name) {
            return Err(RenderError::new(format!(
                "record `{name}` is part of a reference cycle; recursive records cannot \
                 cross the boundary by value"
            )));
        }
        let Some(record) = self.records.get(name) else {
            return Err(RenderError::new(format!(
                "`{name}` is not a #[unibind::record] in this module"
            )));
        };
        stack.push(name.to_owned());
        for field in &record.fields {
            self.check_type(&field.ty, stack)?;
        }
        stack.pop();
        Ok(())
    }

    /// The record behind a validated [`CTy::Record`] name.
    ///
    /// # Panics
    ///
    /// Panics when `name` was not validated by [`Model::new`].
    #[must_use]
    pub fn record(&self, name: &str) -> &'ir ir::Record {
        self.records
            .get(name)
            .expect("record names are validated when the model is built")
    }

    /// Layout of one mirror.
    #[must_use]
    pub fn layout(&self, ty: &CTy) -> Layout {
        match ty {
            CTy::Bool => Layout { size: 1, align: 1 },
            CTy::Int(kind) => ctype::int_layout(*kind),
            CTy::Float(ir::FloatKind::F32) => Layout { size: 4, align: 4 },
            CTy::Float(ir::FloatKind::F64) => Layout { size: 8, align: 8 },
            CTy::Str | CTy::Path | CTy::Bytes | CTy::Vec(_) | CTy::Map { .. } => ctype::PTR_PAIR,
            CTy::Option(inner) => self.option_struct(inner).layout,
            CTy::Record(name) => self.record_struct(name).layout,
        }
    }

    fn option_struct(&self, inner: &CTy) -> StructLayout {
        ctype::struct_layout(&[Layout { size: 1, align: 1 }, self.layout(inner)])
    }

    /// Offset of the payload inside `COption<inner>` (the presence flag is
    /// at 0).
    #[must_use]
    pub fn option_value_offset(&self, inner: &CTy) -> u64 {
        self.option_struct(inner).offsets[1]
    }

    /// Layout of the `CPair<key, value>` entry a map crosses as.
    #[must_use]
    pub fn pair_struct(&self, key: &CTy, value: &CTy) -> StructLayout {
        ctype::struct_layout(&[self.layout(key), self.layout(value)])
    }

    /// Layout of a record's `#[repr(C)]` mirror, offsets aligned with the
    /// record's fields.
    ///
    /// # Panics
    ///
    /// Panics when `name` was not validated by [`Model::new`].
    #[must_use]
    pub fn record_struct(&self, name: &str) -> StructLayout {
        let fields: Vec<Layout> = self
            .record(name)
            .fields
            .iter()
            .map(|field| self.layout(&CTy::of(&field.ty)))
            .collect();
        ctype::struct_layout(&fields)
    }

    /// Layout of a function's return envelope: `code: i32` at 0, then the
    /// `err_msg` text, then the success payload when the function returns
    /// one.
    #[must_use]
    pub fn envelope(&self, ret: Option<&CTy>) -> EnvelopeLayout {
        let mut fields = vec![Layout { size: 4, align: 4 }, ctype::PTR_PAIR];
        if let Some(ret) = ret {
            fields.push(self.layout(ret));
        }
        let shape = ctype::struct_layout(&fields);
        let value_offset = ret.map(|_| shape.offsets[2]);
        EnvelopeLayout {
            layout: shape.layout,
            err_msg_offset: shape.offsets[1],
            value_offset,
        }
    }

    /// Every distinct aggregate mirror reachable from `roots`, keyed and
    /// ordered by mangled name.
    pub fn reachable_aggregates<'ty>(
        &self,
        roots: impl Iterator<Item = &'ty ir::Type>,
    ) -> BTreeMap<String, CTy> {
        let mut found = BTreeMap::new();
        for ty in roots {
            self.visit(&CTy::of(ty), &mut found);
        }
        found
    }

    fn visit(&self, ty: &CTy, found: &mut BTreeMap<String, CTy>) {
        if ty.is_scalar() {
            return;
        }
        let mangle = ty.mangle();
        if found.contains_key(&mangle) {
            return;
        }
        found.insert(mangle, ty.clone());
        match ty {
            // The path helpers decode and encode through the text helpers.
            CTy::Path => self.visit(&CTy::Str, found),
            CTy::Option(inner) | CTy::Vec(inner) => self.visit(inner, found),
            CTy::Map { key, value } => {
                self.visit(key, found);
                self.visit(value, found);
            }
            CTy::Record(name) => {
                for field in &self.record(name).fields {
                    self.visit(&CTy::of(&field.ty), found);
                }
            }
            CTy::Bool | CTy::Int(_) | CTy::Float(_) | CTy::Str | CTy::Bytes => {}
        }
    }
}
