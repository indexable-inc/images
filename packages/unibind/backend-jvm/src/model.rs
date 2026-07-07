//! The validated type model of one interface.
//!
//! [`Model::new`] is the shared gate all three generators pass through: it
//! rejects the surface the JVM backend does not implement (data enums) and
//! everything that cannot cross the boundary (streams outside return
//! position, objects outside handle position, unresolved or recursive
//! records), so the layout queries and the per-record lookups can assume a
//! well-formed interface.

mod validate;

use std::collections::{BTreeMap, BTreeSet};

use unibind_core::ir;

use crate::ctype::{self, CTy, EnvelopeLayout, Layout, StructLayout};
use crate::RenderError;

/// Record lookup plus layout queries for every reachable mirror.
pub struct Model<'ir> {
    records: BTreeMap<&'ir str, &'ir ir::Record>,
    /// Names declared as `#[unibind::object]`; a `Named` return naming one
    /// crosses as an opaque handle instead of a record mirror.
    objects: BTreeSet<&'ir str>,
}

impl<'ir> Model<'ir> {
    /// Validate the JVM surface and build the model.
    ///
    /// # Errors
    ///
    /// Fails for data enums (not rendered yet), for streams or objects in
    /// a position where they cannot cross (arguments, record fields,
    /// nested inside another type), for unresolved or recursive record
    /// types, and for a `throws` name with no matching error enum.
    pub fn new(interface: &'ir ir::Interface) -> Result<Self, RenderError> {
        let model = Self {
            records: interface
                .records
                .iter()
                .map(|record| (record.name.as_str(), record))
                .collect(),
            objects: interface
                .objects
                .iter()
                .map(|object| object.name.as_str())
                .collect(),
        };
        validate::interface(&model, interface)?;
        Ok(model)
    }

    /// Whether `name` is a `#[unibind::object]` in this interface.
    #[must_use]
    pub fn is_object(&self, name: &str) -> bool {
        self.objects.contains(name)
    }

    /// The boundary mirror of one argument or return type: streams and
    /// objects cross as opaque handles, everything else through
    /// [`CTy::of`]. Record fields keep using [`CTy::of`] directly, because
    /// validation rejects streams and objects inside records.
    #[must_use]
    pub fn boundary(&self, ty: &ir::Type) -> CTy {
        match ty {
            ir::Type::Stream(_) => CTy::Handle,
            ir::Type::Named(name) if self.is_object(name) => CTy::Handle,
            _ => CTy::of(ty),
        }
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
            CTy::Float(ir::FloatKind::F64) | CTy::Handle => Layout { size: 8, align: 8 },
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
    /// one. Item envelopes reuse the same shape with a `COption` payload.
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
    /// ordered by mangled name. A stream root contributes `COption` of its
    /// item (the shape its item envelope carries); handles are scalars and
    /// contribute nothing.
    pub fn reachable_aggregates<'ty>(
        &self,
        roots: impl Iterator<Item = &'ty ir::Type>,
    ) -> BTreeMap<String, CTy> {
        let mut found = BTreeMap::new();
        for ty in roots {
            match ty {
                ir::Type::Stream(item) => {
                    self.visit(&CTy::Option(Box::new(CTy::of(item))), &mut found);
                }
                _ => self.visit(&self.boundary(ty), &mut found),
            }
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
            CTy::Bool | CTy::Int(_) | CTy::Float(_) | CTy::Str | CTy::Bytes | CTy::Handle => {}
        }
    }
}
