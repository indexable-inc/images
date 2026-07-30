//! The `.ix` type language: TypeScript type annotations parsed into [`Ty`].
//!
//! [`Ty`] is plain data, and that is the point. The converter used to walk an
//! annotation straight onto a `__ixTy` checker expression and then throw the
//! type away; a checker is a Nix function, so nothing downstream could ask a
//! type what it accepts, and that single missing property is what blocked a
//! generated JSON Schema and a generated `--help` (#4447). One parse now feeds
//! two lowerings that never touch the AST again: [`crate::checker`] emits the
//! eval-time check, [`crate::schema`] emits JSON Schema, and neither can
//! describe a type the other does not.
//!
//! The static half of the type story (tsc against ambient declarations) still
//! never runs here.

use oxc_ast::ast;
use oxc_span::{GetSpan as _, Span};

use crate::error::Error;
use crate::map::{Mapper, Number};

/// One `.ix` type, independent of any output.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    /// `any` / `unknown`: accepts anything, checks nothing.
    Any,
    /// `string`
    Str,
    /// `bool`, and TypeScript's `boolean` keyword.
    Bool,
    /// `int`
    Int,
    /// `float`
    Float,
    /// A range-refined integer: the Rust widths `u8`..`i32`, plus `port`.
    Ranged(IntRange),
    /// `path`: a Nix path value, or an absolute path spelled as a string.
    Path,
    /// `nonEmptyStr`
    NonEmptyStr,
    /// `drv`: a derivation, recognized by its `drvPath` and `outPath` attrs.
    Drv,
    /// A function type. Its parameter and return types cannot be probed
    /// without calling it, so only callability is ever knowable.
    Func,
    /// `T[]`
    List(Box<Self>),
    /// `Record<string, T>`, which `object` spells as `Record<string, any>`.
    Attrs(Box<Self>),
    /// `{ a: T; b?: U }`
    Object(Vec<Field>),
    /// A union of literals: `"tcp" | "udp"`.
    Enum(Vec<Literal>),
    /// `T | null`
    Nullable(Box<Self>),
    /// A reference to one of this module's `type` aliases.
    Alias(String),
}

/// One member of an object type.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
    /// `b?: U`. What "optional" then means depends on the position, so it
    /// stays a flag here: a JSON Schema drops the field from `required`,
    /// while a destructured parameter's bound name is `U | null` because its
    /// Nix default binds when the caller omits the field.
    pub optional: bool,
}

/// A value a literal type pins.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// A range-refined integer: the `ix-ty.nix` checker's name and the inclusive
/// bounds it enforces.
///
/// The bounds live here as well as in the runtime because a JSON Schema has to
/// restate them as `minimum`/`maximum`; the two copies are pinned together by
/// `int_range_bounds_match_ix_ty_nix` below, since a schema that disagreed
/// with the checker would green-light a params file that fails at eval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntRange {
    pub name: &'static str,
    pub min: i64,
    pub max: i64,
}

impl IntRange {
    const fn new(name: &'static str, min: i64, max: i64) -> Self {
        Self { name, min, max }
    }
}

/// Every range-refined integer, in the spelling `.ix` source uses.
pub const INT_RANGES: [IntRange; 7] = [
    IntRange::new("u8", 0, 255),
    IntRange::new("u16", 0, 65535),
    IntRange::new("u32", 0, 4_294_967_295),
    IntRange::new("i8", -128, 127),
    IntRange::new("i16", -32768, 32767),
    IntRange::new("i32", -2_147_483_648, 2_147_483_647),
    IntRange::new("port", 0, 65535),
];

/// Type names with a fixed meaning; `type` aliases may not shadow them
/// ([`crate::map`] rejects the declaration), so a reference is never
/// ambiguous between a built-in and a module alias.
pub const BUILTIN_TYPES: [&str; 14] = [
    "bool",
    "int",
    "float",
    "u8",
    "u16",
    "u32",
    "i8",
    "i16",
    "i32",
    "port",
    "path",
    "nonEmptyStr",
    "drv",
    "Record",
];

/// A module's introspectable types: everything a schema or a `--help`
/// renderer needs about one `.ix` file, produced by the same pass that emits
/// the Nix.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleTypes {
    /// `type X = T` aliases, in declaration order. A [`Ty::Alias`] names one.
    pub aliases: Vec<TypeAlias>,
    /// The parameters of the module's `export default` arrow itself, in
    /// curried call order. Empty when the default export is not an arrow.
    ///
    /// Only that one arrow: a function nested in the value it returns (a
    /// template under a `templates` attrset, say) is not described here, so a
    /// `--help` renderer cannot reach it yet. See #4453.
    pub parameters: Vec<Parameter>,
}

/// One `type X = T` declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAlias {
    pub name: String,
    pub ty: Ty,
}

/// One curried parameter of a module's default export.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    /// The binder as written, or `None` for a destructured parameter: Nix's
    /// `{ a, b }:` binds the fields, never the set.
    pub name: Option<String>,
    /// The annotation, when the parameter carries one.
    pub ty: Option<Ty>,
}

impl Mapper<'_> {
    /// Parses one TypeScript type annotation into its [`Ty`].
    pub(crate) fn ty(&self, t: &ast::TSType<'_>) -> Result<Ty, Error> {
        match t {
            ast::TSType::TSAnyKeyword(_) | ast::TSType::TSUnknownKeyword(_) => Ok(Ty::Any),
            ast::TSType::TSStringKeyword(_) => Ok(Ty::Str),
            ast::TSType::TSBooleanKeyword(_) => Ok(Ty::Bool),
            // `object` is any attrset: `Record<string, any>`, one meaning for
            // both spellings.
            ast::TSType::TSObjectKeyword(_) => Ok(Ty::Attrs(Box::new(Ty::Any))),
            ast::TSType::TSNumberKeyword(number) => Err(self.err(
                number.span,
                "Nix distinguishes integers from floats; use `int` or `float`",
            )),
            ast::TSType::TSNullKeyword(null) => Err(self.err(
                null.span,
                "`null` is only checkable in a union: `T | null`",
            )),
            ast::TSType::TSUndefinedKeyword(undefined) => Err(self.err(
                undefined.span,
                "`undefined` has no Nix equivalent; use `T | null`",
            )),
            ast::TSType::TSArrayType(array) => {
                Ok(Ty::List(Box::new(self.ty(&array.element_type)?)))
            }
            ast::TSType::TSFunctionType(_) => Ok(Ty::Func),
            ast::TSType::TSParenthesizedType(paren) => self.ty(&paren.type_annotation),
            ast::TSType::TSTypeLiteral(literal) => Ok(Ty::Object(self.object_fields(literal)?)),
            ast::TSType::TSUnionType(union) => self.union(union),
            ast::TSType::TSLiteralType(literal) => Ok(Ty::Enum(vec![self.ty_literal(literal)?])),
            ast::TSType::TSTypeReference(reference) => self.reference(reference),
            other => Err(self.err(
                other.span(),
                "this type has no runtime lowering; see the ix2nix type table",
            )),
        }
    }

    /// The members of `{ a: T; b?: U }`, in declaration order.
    ///
    /// Duplicate names are rejected rather than kept: two `a` fields have no
    /// sensible checker (the runtime would demand `a` twice) and no sensible
    /// schema (JSON object keys are unique, so one would silently win).
    pub(crate) fn object_fields(
        &self,
        literal: &ast::TSTypeLiteral<'_>,
    ) -> Result<Vec<Field>, Error> {
        let mut fields: Vec<Field> = Vec::with_capacity(literal.members.len());
        for member in &literal.members {
            let field = self.field(member)?;
            self.reject_duplicate_field(&fields, &field, member.span())?;
            fields.push(field);
        }
        Ok(fields)
    }

    /// Rejects a field name already among `fields`.
    pub(crate) fn reject_duplicate_field(
        &self,
        fields: &[Field],
        field: &Field,
        span: Span,
    ) -> Result<(), Error> {
        if fields.iter().any(|seen| seen.name == field.name) {
            return Err(self.err(
                span,
                format!("duplicate field `{}` in object type", field.name),
            ));
        }
        Ok(())
    }

    /// One `a: T` / `b?: U` member of an object type.
    pub(crate) fn field(&self, member: &ast::TSSignature<'_>) -> Result<Field, Error> {
        let ast::TSSignature::TSPropertySignature(property) = member else {
            return Err(self.err(
                member.span(),
                "only property signatures lower; index, call, and method signatures have no runtime check",
            ));
        };
        let Some(annotation) = &property.type_annotation else {
            return Err(self.err(property.span, "property signature needs a type"));
        };
        if property.computed {
            return Err(self.err(property.span, "computed keys have no runtime check"));
        }
        // Rejected rather than ignored, for the crate's usual reason: a form
        // with no Nix meaning is an error, not a no-op. Every Nix attrset is
        // immutable, so `readonly` asserts nothing a reader could act on.
        //
        // Load-bearing beyond that: `readonly` is the only modifier a property
        // signature can carry, so refusing it is what keeps a member's span
        // starting at its key. `crate::map` reports a destructured field's
        // check location from the member span and relies on the two agreeing.
        if property.readonly {
            return Err(self.err(
                property.span,
                "`readonly` has no Nix equivalent; every attrset is already immutable",
            ));
        }
        let name = match &property.key {
            ast::PropertyKey::StaticIdentifier(ident) => ident.name.to_string(),
            ast::PropertyKey::StringLiteral(lit) => lit.value.to_string(),
            other => {
                return Err(self.err(other.span(), "property key must be a name or string"));
            }
        };
        Ok(Field {
            name,
            ty: self.ty(&annotation.type_annotation)?,
            optional: property.optional,
        })
    }

    /// Unions parse in exactly two shapes: `T | null` and literal enums
    /// (optionally `| null`). Anything else has no single runtime check.
    fn union(&self, union: &ast::TSUnionType<'_>) -> Result<Ty, Error> {
        let mut nullable = false;
        let mut literals = Vec::new();
        let mut others = Vec::new();
        for member in &union.types {
            match member {
                ast::TSType::TSNullKeyword(_) => nullable = true,
                ast::TSType::TSLiteralType(literal) => literals.push(self.ty_literal(literal)?),
                other => others.push(other),
            }
        }
        let inner = match (literals.is_empty(), others.as_slice()) {
            (false, []) => Ty::Enum(literals),
            (true, [single]) => self.ty(single)?,
            _ => {
                return Err(self.err(
                    union.span,
                    "unions lower only as `T | null` or a union of literals",
                ));
            }
        };
        Ok(if nullable {
            Ty::Nullable(Box::new(inner))
        } else {
            inner
        })
    }

    fn ty_literal(&self, literal: &ast::TSLiteralType<'_>) -> Result<Literal, Error> {
        match &literal.literal {
            ast::TSLiteral::StringLiteral(lit) => Ok(Literal::Str(lit.value.to_string())),
            // Shares the value mapper's numeric parsing (radix prefixes, `_`
            // separators, the 64-bit range check), so a literal type and a
            // literal value can never disagree about what a number is.
            ast::TSLiteral::NumericLiteral(lit) => Ok(match self.number(lit)? {
                Number::Int(value) => Literal::Int(value),
                Number::Float(value) => Literal::Float(value),
            }),
            ast::TSLiteral::BooleanLiteral(lit) => Ok(Literal::Bool(lit.value)),
            other => Err(self.err(other.span(), "this literal type has no runtime check")),
        }
    }

    /// Named types: the built-ins and in-module `type` aliases. Everything
    /// else is unknown here; ambient declaration files are tsc's world, not
    /// the converter's.
    fn reference(&self, reference: &ast::TSTypeReference<'_>) -> Result<Ty, Error> {
        let ast::TSTypeName::IdentifierReference(ident) = &reference.type_name else {
            return Err(self.err(
                reference.span,
                "qualified type names have no runtime lowering",
            ));
        };
        let name = ident.name.as_str();
        if name == "Record" {
            let Some(arguments) = &reference.type_arguments else {
                return Err(self.err(reference.span, "`Record` needs `<string, T>` arguments"));
            };
            let [key, value] = arguments.params.as_slice() else {
                return Err(self.err(arguments.span, "`Record` takes exactly two arguments"));
            };
            if !matches!(key, ast::TSType::TSStringKeyword(_)) {
                return Err(self.err(
                    key.span(),
                    "Nix attrset keys are strings; use `Record<string, T>`",
                ));
            }
            return Ok(Ty::Attrs(Box::new(self.ty(value)?)));
        }
        if let Some(arguments) = &reference.type_arguments {
            return Err(self.err(arguments.span, "generic types are not lowered yet"));
        }
        if let Some(range) = INT_RANGES.iter().find(|range| range.name == name) {
            return Ok(Ty::Ranged(*range));
        }
        // Lowercase on purpose: `int` and `float` are what Nix itself calls
        // the types, the width refinements are Rust vocabulary, and `port` /
        // `path` / `nonEmptyStr` come from nixpkgs `lib.types`. TypeScript's
        // bare `number` stays banned.
        match name {
            // TypeScript's keyword is `boolean`; Nix's name is `bool`. The
            // keyword arrives as TSBooleanKeyword, this arm catches the Nix
            // spelling, and both parse to the same type.
            "bool" => Ok(Ty::Bool),
            "int" => Ok(Ty::Int),
            "float" => Ok(Ty::Float),
            "path" => Ok(Ty::Path),
            "nonEmptyStr" => Ok(Ty::NonEmptyStr),
            "drv" => Ok(Ty::Drv),
            _ if self.type_aliases.contains(name) => Ok(Ty::Alias(name.to_owned())),
            _ => Err(self.err(
                ident.span,
                format!(
                    "unknown type `{name}`; built-ins are int, float, \
                     u8/u16/u32/i8/i16/i32, port, path, nonEmptyStr, drv, and \
                     Record<string, T>, plus this module's `type` aliases"
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BUILTIN_TYPES, INT_RANGES};

    /// A refined integer's bounds are stated twice -- once here for the JSON
    /// Schema's `minimum`/`maximum`, once in `ix-ty.nix` for the eval-time
    /// check -- so they are pinned to each other. A schema wider than the
    /// checker green-lights a params file that then fails at eval; narrower,
    /// and an editor rejects a value Nix accepts.
    #[test]
    fn int_range_bounds_match_ix_ty_nix() {
        let runtime = include_str!("../ix-ty.nix");
        for range in INT_RANGES {
            // Nix parenthesizes a negative literal in application position.
            let nix_int = |value: i64| {
                if value < 0 {
                    format!("({value})")
                } else {
                    value.to_string()
                }
            };
            let call = format!(
                "intIn \"{}\" {} {}",
                range.name,
                nix_int(range.min),
                nix_int(range.max)
            );
            assert!(
                runtime.contains(&call),
                "ix-ty.nix does not define `{call}`; the schema's bounds for \
                 `{}` disagree with the runtime's",
                range.name
            );
        }
    }

    /// A `type` alias shadowing a built-in is rejected by name, so every
    /// refined-integer spelling has to appear in that list too.
    #[test]
    fn every_int_range_is_an_unshadowable_builtin() {
        for range in INT_RANGES {
            assert!(
                BUILTIN_TYPES.contains(&range.name),
                "`{}` is a built-in type but `type {}` is not rejected",
                range.name,
                range.name
            );
        }
    }
}
