//! Compiled modules as content-addressed objects, and the compile cache over
//! them.
//!
//! `ir::Module` was designed to be serializable -- indices rather than
//! pointers, a const pool of pure literals -- and this is the module that
//! cashes that in. A `Module` encodes to the kernel's canonical CBOR subset,
//! which gives it a byte string that is a function of the module alone, hence
//! an `ObjId`, hence a row in the memo table.
//!
//! # What the key covers, and why it covers so much
//!
//! `H(format version, compiler fingerprint, base directory, origin, the
//! settings the compiler reads, source text)`.
//!
//! The source text is obvious. The other three are the ones that go wrong if
//! left out:
//!
//! * The **base directory** changes the module. `compile_source` makes path
//!   literals absolute against it, so the same text in two directories
//!   compiles to two different const pools.
//! * The **compiler fingerprint** is a hash of every source file in this
//!   crate, not just the compiler's. `Op::Builtin { idx }` and
//!   `Op::CallBuiltin { idx }` hold indices into the builtin table, so
//!   inserting a builtin silently changes what an already-compiled op means.
//!   Hashing the whole crate over-invalidates and never under-invalidates,
//!   which is the only direction a cache is allowed to be wrong in.
//! * The **settings the compiler reads** are the names cppnix's own `builtins`
//!   has, which decide which bare globals resolve. The same text compiles to
//!   `undefined variable '__fetchClosure'` under one configuration and to a
//!   builtin reference under another.
//! * The **origin** is the file the text came from, which `__curPos` compiles
//!   to a constant of. The base directory is not enough: two files in one
//!   directory with the same text are one key by base directory and two
//!   different `__curPos` answers.
//! * The **format version** separates a change in this file's encoding from a
//!   change in what is being encoded.
//!
//! Nothing here is a trust decision: the policy is [`Policy::Keyed`], meaning
//! re-performing is always safe, so a miss costs a compile and never a wrong
//! answer.

use crate::ir::{AttrSite, CodeUnit, Const, Formal, Module, NO_POS, Op, Param, SrcOrigin};
use crate::refusal::{Refusal, RefusalToken};
use ix_kernel::canon::{self, CanonValue, DecodeError};
use ix_kernel::cas::Cas;
use ix_kernel::dispatch::{Outcome, PerformCtx, on_perform};
use ix_kernel::rows::{DirRows, Lookup};
use ix_kernel::{Domain, EffectLock, KernelConfig, KernelError, MemoTable, ObjId, Policy};
use std::rc::Rc;

/// Bumped when this file's encoding changes shape. Distinct from the compiler
/// fingerprint, which changes when the thing being encoded changes.
pub const MODULE_FORMAT_VERSION: &str = "ixe-module-v1";

/// Hash of every source file in this crate, computed at build time. See the
/// module header for why the whole crate and not just the compiler.
#[must_use]
pub fn compiler_fingerprint() -> &'static str {
    env!("IXE_COMPILER_FINGERPRINT")
}

/// The effect this cache memoises.
#[must_use]
pub fn compile_domain() -> Domain {
    Domain::mint("ix-eval.compile", "module")
}

/// Why a stored object was not a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleDecodeError {
    /// The bytes were not a canonical encoding at all.
    Canon(DecodeError),
    /// They were, but not of a module: a field missing, of the wrong kind, or
    /// carrying a tag this format version does not define.
    Shape(String),
}

impl core::fmt::Display for ModuleDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Canon(source) => write!(f, "not a canonical encoding: {source}"),
            Self::Shape(detail) => write!(f, "not a {MODULE_FORMAT_VERSION} module: {detail}"),
        }
    }
}

impl core::error::Error for ModuleDecodeError {}

fn shape<T>(detail: impl Into<String>) -> Result<T, ModuleDecodeError> {
    Err(ModuleDecodeError::Shape(detail.into()))
}

// ---------------------------------------------------------------- encoding

/// Encode a module into the canonical subset.
///
/// Every `match` below is exhaustive with no wildcard arm, which is the guard
/// against silent format drift: adding a variant to `Op`, `Const` or `Param`
/// fails to compile here rather than encoding as something else.
pub fn encode_module(module: &Module) -> Result<Vec<u8>, canon::CanonError> {
    canon::encode(&module_value(module))
}

fn module_value(module: &Module) -> CanonValue {
    CanonValue::map([
        (
            "consts",
            CanonValue::Array(module.consts.iter().map(const_value).collect()),
        ),
        (
            "symbols",
            CanonValue::Array(
                module
                    .symbols
                    .iter()
                    .map(|s| CanonValue::str(s.as_str()))
                    .collect(),
            ),
        ),
        (
            "units",
            CanonValue::Array(module.units.iter().map(unit_value).collect()),
        ),
        ("entry", CanonValue::int(module.entry)),
        (
            "line_starts",
            CanonValue::Array(
                module
                    .line_starts
                    .iter()
                    .map(|n| CanonValue::int(*n))
                    .collect(),
            ),
        ),
        (
            "origin",
            match &module.origin {
                SrcOrigin::String => CanonValue::array([CanonValue::int(0)]),
                SrcOrigin::File(path) => {
                    CanonValue::array([CanonValue::int(1), CanonValue::str(path.as_str())])
                }
            },
        ),
    ])
}

fn const_value(konst: &Const) -> CanonValue {
    match konst {
        Const::Int(n) => CanonValue::array([CanonValue::int(0), CanonValue::Int(i128::from(*n))]),
        // The canonical subset has no float, by design. Carrying the IEEE-754
        // bits keeps the encoding injective (and NaN payloads intact), which a
        // decimal rendering would not.
        Const::Float(f) => CanonValue::array([
            CanonValue::int(1),
            CanonValue::Bytes(f.to_bits().to_be_bytes().to_vec()),
        ]),
        Const::Bool(b) => CanonValue::array([CanonValue::int(2), CanonValue::Bool(*b)]),
        Const::Null => CanonValue::array([CanonValue::int(3)]),
        Const::Str(s) => CanonValue::array([CanonValue::int(4), CanonValue::str(s.as_str())]),
        Const::Path(p) => CanonValue::array([CanonValue::int(5), CanonValue::str(p.as_str())]),
    }
}

fn unit_value(unit: &CodeUnit) -> CanonValue {
    let param = match &unit.param {
        None => CanonValue::Null,
        Some(Param::Ident(sym)) => CanonValue::array([CanonValue::int(0), CanonValue::int(*sym)]),
        Some(Param::Formals {
            fields,
            ellipsis,
            bind,
        }) => CanonValue::array([
            CanonValue::int(1),
            CanonValue::Array(
                fields
                    .iter()
                    .map(|f| {
                        CanonValue::array([
                            CanonValue::int(f.sym),
                            f.default.map_or(CanonValue::Null, CanonValue::int),
                            CanonValue::int(f.pos),
                        ])
                    })
                    .collect(),
            ),
            CanonValue::Bool(*ellipsis),
            bind.map_or(CanonValue::Null, CanonValue::int),
        ]),
    };
    CanonValue::map([
        (
            "ops",
            CanonValue::Array(unit.ops.iter().map(op_value).collect()),
        ),
        ("param", param),
        (
            "spans",
            CanonValue::Array(unit.spans.iter().map(|n| CanonValue::int(*n)).collect()),
        ),
        (
            "attr_sites",
            CanonValue::Array(
                unit.attr_sites
                    .iter()
                    .map(|site| {
                        CanonValue::array([
                            CanonValue::int(site.ip),
                            CanonValue::Array(
                                site.names
                                    .iter()
                                    .map(|(sym, pos)| {
                                        CanonValue::array([
                                            CanonValue::int(*sym),
                                            CanonValue::int(*pos),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// Tags are written out rather than derived from the variant's position, so
/// reordering `Op` cannot change the format under an unchanged version string.
fn op_value(op: &Op) -> CanonValue {
    let nullary = |tag: i128| CanonValue::array([CanonValue::Int(tag)]);
    let unary = |tag: i128, a: u32| CanonValue::array([CanonValue::Int(tag), CanonValue::int(a)]);
    match *op {
        Op::Const(idx) => unary(1, idx),
        Op::GetLocal { depth, slot } => CanonValue::array([
            CanonValue::int(2),
            CanonValue::int(depth),
            CanonValue::int(slot),
        ]),
        Op::GetLocalLazy { depth, slot } => CanonValue::array([
            CanonValue::int(3),
            CanonValue::int(depth),
            CanonValue::int(slot),
        ]),
        Op::Builtin { idx } => unary(4, u32::from(idx)),
        Op::BuiltinsSet => nullary(5),
        Op::UnimplementedGlobal { sym } => unary(6, sym),
        Op::DerivationGlobal => nullary(43),
        Op::NixPathGlobal => nullary(44),
        Op::Thunk { unit } => unary(7, unit),
        Op::Closure { unit } => unary(8, unit),
        Op::Apply => nullary(9),
        Op::PushEnv { n } => unary(10, u32::from(n)),
        Op::PopEnv => nullary(11),
        Op::JumpIfFalse { target } => unary(12, target),
        Op::Jump { target } => unary(13, target),
        Op::Add => nullary(14),
        Op::Sub => nullary(15),
        Op::Mul => nullary(16),
        Op::Div => nullary(17),
        Op::Eq => nullary(18),
        Op::Neq => nullary(19),
        Op::Lt => nullary(20),
        Op::Leq => nullary(21),
        Op::Gt => nullary(22),
        Op::Geq => nullary(23),
        Op::Not => nullary(24),
        Op::Negate => nullary(25),
        Op::ConcatStrings { n } => unary(26, u32::from(n)),
        Op::MkList { n } => unary(27, u32::from(n)),
        Op::ConcatLists => nullary(28),
        Op::MkAttrs { n, rec } => CanonValue::array([
            CanonValue::int(29),
            CanonValue::int(n),
            CanonValue::Bool(rec),
        ]),
        Op::Update => nullary(30),
        Op::Select { sym } => unary(31, sym),
        Op::SelectSoft { sym } => unary(32, sym),
        Op::SelectSoftDyn => nullary(33),
        Op::OrDefault => nullary(34),
        Op::HasAttr { sym } => unary(35, sym),
        Op::SelectDyn => nullary(36),
        Op::HasAttrDyn => nullary(37),
        Op::PushWith => nullary(38),
        Op::ResolveWith { sym } => unary(39, sym),
        Op::CallBuiltin { idx } => unary(40, u32::from(idx)),
        // 45, the next free tag, and not a reuse of 26: the tag is the wire
        // format, so a module compiled before this op existed must still
        // decode. `every_tag_is_used_once` below is what says which tags are
        // taken -- reading the encoders and taking the largest missed the
        // nullary spellings and picked 43, which `DerivationGlobal` holds.
        Op::ConcatPath { n } => unary(45, u32::from(n)),
        // 46, the next free tag; see the note on 45.
        Op::MkAttrsOnto { n } => unary(46, u32::from(n)),
        Op::Assert => nullary(41),
        Op::Ret => nullary(42),
    }
}

// ---------------------------------------------------------------- decoding

/// Decode a module from canonical bytes.
pub fn decode_module(bytes: &[u8]) -> Result<Module, ModuleDecodeError> {
    let value = canon::decode(bytes).map_err(ModuleDecodeError::Canon)?;
    module_from(&value)
}

fn field<'a>(value: &'a CanonValue, name: &str) -> Result<&'a CanonValue, ModuleDecodeError> {
    let CanonValue::Map(entries) = value else {
        return shape(format!("expected a map to take '{name}' from"));
    };
    entries
        .iter()
        .find(|(key, _)| matches!(key, CanonValue::Str(k) if k == name))
        .map(|(_, v)| v)
        .ok_or_else(|| ModuleDecodeError::Shape(format!("missing field '{name}'")))
}

fn items<'a>(value: &'a CanonValue, what: &str) -> Result<&'a [CanonValue], ModuleDecodeError> {
    match value {
        CanonValue::Array(items) => Ok(items),
        _ => shape(format!("expected an array for {what}")),
    }
}

fn small(value: &CanonValue, what: &str) -> Result<u32, ModuleDecodeError> {
    match value {
        CanonValue::Int(n) => u32::try_from(*n)
            .map_err(|_| ModuleDecodeError::Shape(format!("{what} out of range: {n}"))),
        _ => shape(format!("expected an integer for {what}")),
    }
}

fn narrow(value: &CanonValue, what: &str) -> Result<u16, ModuleDecodeError> {
    u16::try_from(small(value, what)?)
        .map_err(|_| ModuleDecodeError::Shape(format!("{what} does not fit in u16")))
}

fn boolean(value: &CanonValue, what: &str) -> Result<bool, ModuleDecodeError> {
    match value {
        CanonValue::Bool(b) => Ok(*b),
        _ => shape(format!("expected a bool for {what}")),
    }
}

fn text(value: &CanonValue, what: &str) -> Result<String, ModuleDecodeError> {
    match value {
        CanonValue::Str(s) => Ok(s.clone()),
        _ => shape(format!("expected a string for {what}")),
    }
}

fn optional(value: &CanonValue, what: &str) -> Result<Option<u32>, ModuleDecodeError> {
    match value {
        CanonValue::Null => Ok(None),
        other => Ok(Some(small(other, what)?)),
    }
}

/// Positional read with a name, so a short array reports which operand is
/// missing rather than panicking on an index.
fn at<'a>(
    items: &'a [CanonValue],
    index: usize,
    what: &str,
) -> Result<&'a CanonValue, ModuleDecodeError> {
    items
        .get(index)
        .ok_or_else(|| ModuleDecodeError::Shape(format!("{what}: no element {index}")))
}

fn module_from(value: &CanonValue) -> Result<Module, ModuleDecodeError> {
    let consts = items(field(value, "consts")?, "consts")?
        .iter()
        .map(const_from)
        .collect::<Result<Vec<_>, _>>()?;
    let symbols = items(field(value, "symbols")?, "symbols")?
        .iter()
        .map(|s| text(s, "symbol"))
        .collect::<Result<Vec<_>, _>>()?;
    let units = items(field(value, "units")?, "units")?
        .iter()
        .map(unit_from)
        .collect::<Result<Vec<_>, _>>()?;
    let entry = small(field(value, "entry")?, "entry")?;
    // A module whose entry names no unit would fault the first time it ran,
    // far from the cache that handed it over.
    if entry as usize >= units.len() {
        return shape(format!(
            "entry unit {entry} but the module has {} units",
            units.len()
        ));
    }
    let line_starts = items(field(value, "line_starts")?, "line starts")?
        .iter()
        .map(|n| small(n, "line start"))
        .collect::<Result<Vec<_>, _>>()?;
    let origin_parts = items(field(value, "origin")?, "origin")?;
    let origin = match small(at(origin_parts, 0, "origin tag")?, "origin tag")? {
        0 => SrcOrigin::String,
        1 => SrcOrigin::File(text(at(origin_parts, 1, "origin path")?, "origin path")?),
        other => return shape(format!("unknown origin tag {other}")),
    };
    Ok(Module {
        consts,
        symbols,
        units,
        entry,
        origin,
        line_starts,
    })
}

fn const_from(value: &CanonValue) -> Result<Const, ModuleDecodeError> {
    let parts = items(value, "const")?;
    match small(at(parts, 0, "const tag")?, "const tag")? {
        0 => match at(parts, 1, "int const")? {
            CanonValue::Int(n) => i64::try_from(*n)
                .map(Const::Int)
                .map_err(|_| ModuleDecodeError::Shape(format!("int const out of range: {n}"))),
            _ => shape("int const is not an integer"),
        },
        1 => match at(parts, 1, "float const")? {
            CanonValue::Bytes(raw) => <[u8; 8]>::try_from(raw.as_slice())
                .map(|bits| Const::Float(f64::from_bits(u64::from_be_bytes(bits))))
                .map_err(|_| {
                    ModuleDecodeError::Shape(format!("float const is {} bytes, not 8", raw.len()))
                }),
            _ => shape("float const is not a byte string"),
        },
        2 => Ok(Const::Bool(boolean(
            at(parts, 1, "bool const")?,
            "bool const",
        )?)),
        3 => Ok(Const::Null),
        4 => Ok(Const::Str(text(at(parts, 1, "str const")?, "str const")?)),
        5 => Ok(Const::Path(text(
            at(parts, 1, "path const")?,
            "path const",
        )?)),
        other => shape(format!("unknown const tag {other}")),
    }
}

fn unit_from(value: &CanonValue) -> Result<CodeUnit, ModuleDecodeError> {
    let ops = items(field(value, "ops")?, "ops")?
        .iter()
        .map(op_from)
        .collect::<Result<Vec<_>, _>>()?;
    let param = param_from(field(value, "param")?)?;
    let spans = items(field(value, "spans")?, "spans")?
        .iter()
        .map(|n| small(n, "span"))
        .collect::<Result<Vec<_>, _>>()?;
    // A decoder that quietly handed back a short table would make every
    // position in this unit read as the position of some other op, which is
    // worse than no position at all.
    if spans.len() != ops.len() {
        return shape(format!(
            "{} spans for {} ops; the side table is not parallel to the code",
            spans.len(),
            ops.len()
        ));
    }
    let attr_sites = items(field(value, "attr_sites")?, "attr sites")?
        .iter()
        .map(|site| {
            let parts = items(site, "attr site")?;
            let names = items(at(parts, 1, "attr site names")?, "attr site names")?
                .iter()
                .map(|pair| {
                    let pair = items(pair, "attr site name")?;
                    Ok((
                        small(at(pair, 0, "attr name symbol")?, "attr name symbol")?,
                        small(at(pair, 1, "attr name position")?, "attr name position")?,
                    ))
                })
                .collect::<Result<Vec<_>, ModuleDecodeError>>()?;
            Ok(AttrSite {
                ip: small(at(parts, 0, "attr site ip")?, "attr site ip")?,
                names,
            })
        })
        .collect::<Result<Vec<_>, ModuleDecodeError>>()?;
    Ok(CodeUnit {
        ops,
        param,
        spans,
        attr_sites,
    })
}

fn param_from(value: &CanonValue) -> Result<Option<Param>, ModuleDecodeError> {
    if matches!(value, CanonValue::Null) {
        return Ok(None);
    }
    let parts = items(value, "param")?;
    match small(at(parts, 0, "param tag")?, "param tag")? {
        0 => Ok(Some(Param::Ident(small(
            at(parts, 1, "ident param")?,
            "ident param",
        )?))),
        1 => {
            let fields = items(at(parts, 1, "formals")?, "formals")?
                .iter()
                .map(|entry| {
                    let triple = items(entry, "formal")?;
                    Ok(Formal {
                        sym: small(at(triple, 0, "formal name")?, "formal name")?,
                        default: optional(at(triple, 1, "formal default")?, "formal default")?,
                        pos: at(triple, 2, "formal position")
                            .and_then(|v| small(v, "formal position"))
                            .unwrap_or(NO_POS),
                    })
                })
                .collect::<Result<Vec<_>, ModuleDecodeError>>()?;
            Ok(Some(Param::Formals {
                fields,
                ellipsis: boolean(at(parts, 2, "ellipsis")?, "ellipsis")?,
                bind: optional(at(parts, 3, "@-binding")?, "@-binding")?,
            }))
        }
        other => shape(format!("unknown param tag {other}")),
    }
}

fn op_from(value: &CanonValue) -> Result<Op, ModuleDecodeError> {
    let parts = items(value, "op")?;
    let tag = small(at(parts, 0, "op tag")?, "op tag")?;
    let one = |what: &str| small(at(parts, 1, what)?, what);
    let one_narrow = |what: &str| narrow(at(parts, 1, what)?, what);
    match tag {
        1 => Ok(Op::Const(one("const index")?)),
        2 => Ok(Op::GetLocal {
            depth: narrow(at(parts, 1, "depth")?, "depth")?,
            slot: narrow(at(parts, 2, "slot")?, "slot")?,
        }),
        3 => Ok(Op::GetLocalLazy {
            depth: narrow(at(parts, 1, "depth")?, "depth")?,
            slot: narrow(at(parts, 2, "slot")?, "slot")?,
        }),
        4 => Ok(Op::Builtin {
            idx: one_narrow("builtin index")?,
        }),
        5 => Ok(Op::BuiltinsSet),
        6 => Ok(Op::UnimplementedGlobal {
            sym: one("symbol")?,
        }),
        43 => Ok(Op::DerivationGlobal),
        44 => Ok(Op::NixPathGlobal),
        7 => Ok(Op::Thunk { unit: one("unit")? }),
        8 => Ok(Op::Closure { unit: one("unit")? }),
        9 => Ok(Op::Apply),
        10 => Ok(Op::PushEnv {
            n: one_narrow("frame size")?,
        }),
        11 => Ok(Op::PopEnv),
        12 => Ok(Op::JumpIfFalse {
            target: one("target")?,
        }),
        13 => Ok(Op::Jump {
            target: one("target")?,
        }),
        14 => Ok(Op::Add),
        15 => Ok(Op::Sub),
        16 => Ok(Op::Mul),
        17 => Ok(Op::Div),
        18 => Ok(Op::Eq),
        19 => Ok(Op::Neq),
        20 => Ok(Op::Lt),
        21 => Ok(Op::Leq),
        22 => Ok(Op::Gt),
        23 => Ok(Op::Geq),
        24 => Ok(Op::Not),
        25 => Ok(Op::Negate),
        26 => Ok(Op::ConcatStrings {
            n: one_narrow("string count")?,
        }),
        27 => Ok(Op::MkList {
            n: one_narrow("list length")?,
        }),
        28 => Ok(Op::ConcatLists),
        29 => Ok(Op::MkAttrs {
            n: narrow(at(parts, 1, "attr count")?, "attr count")?,
            rec: boolean(at(parts, 2, "rec")?, "rec")?,
        }),
        30 => Ok(Op::Update),
        31 => Ok(Op::Select {
            sym: one("symbol")?,
        }),
        32 => Ok(Op::SelectSoft {
            sym: one("symbol")?,
        }),
        33 => Ok(Op::SelectSoftDyn),
        34 => Ok(Op::OrDefault),
        35 => Ok(Op::HasAttr {
            sym: one("symbol")?,
        }),
        36 => Ok(Op::SelectDyn),
        37 => Ok(Op::HasAttrDyn),
        38 => Ok(Op::PushWith),
        39 => Ok(Op::ResolveWith {
            sym: one("symbol")?,
        }),
        40 => Ok(Op::CallBuiltin {
            idx: one_narrow("builtin index")?,
        }),
        41 => Ok(Op::Assert),
        42 => Ok(Op::Ret),
        45 => Ok(Op::ConcatPath {
            n: one_narrow("path segment count")?,
        }),
        46 => Ok(Op::MkAttrsOnto {
            n: one_narrow("attr count")?,
        }),
        other => shape(format!("unknown op tag {other}")),
    }
}

// ------------------------------------------------------------------- cache

/// The request a compile is keyed by. Encoded canonically, so the key is a
/// function of these four fields and nothing else.
fn request(
    base_dir: &str,
    source: &str,
    origin: crate::compile::Origin<'_>,
    settings: &crate::eval::Settings,
) -> CanonValue {
    CanonValue::map([
        ("format", CanonValue::str(MODULE_FORMAT_VERSION)),
        ("compiler", CanonValue::str(compiler_fingerprint())),
        ("base_dir", CanonValue::str(base_dir)),
        // Which primops cppnix registered is configuration, and the compiler
        // reads it: a name cppnix skipped is not a global, so the same text
        // compiles to `undefined variable '__fetchClosure'` under one
        // configuration and to a builtin reference under another
        // (ENG-12717). Without this the second would be served the first
        // one's module.
        (
            "cpp_builtins",
            CanonValue::str(settings.cpp_builtin_names.as_deref().unwrap_or("")),
        ),
        // The other half of the same rule, and it was missing. `pure-eval`
        // reaches the compiler through the same `primop_registered` the line
        // above is about: under it, cppnix registers no impure-only constant,
        // so `__currentSystem` is `undefined variable` rather than a global
        // reference. A key carrying `cpp_builtins` and not this one would
        // serve a module compiled under impure evaluation to a pure one, and
        // the served module resolves a name cppnix refuses. ENG-12939.
        (
            "pure_eval",
            CanonValue::str(if settings.pure_eval { "1" } else { "0" }),
        ),
        // `__curPos` compiles to the file name of the token, so the same text
        // at two paths is two modules. Without this the second path would be
        // served the first one's constant, and `meta.position` is a string
        // that reaches derivations (ENG-12713).
        (
            "origin",
            CanonValue::str(match origin {
                crate::compile::Origin::String => "",
                crate::compile::Origin::File(path) => path,
            }),
        ),
        // The source text itself rather than a hash of it: the kernel hashes
        // the encoded request anyway, so hashing here would only add a second
        // hashing convention to explain.
        ("source", CanonValue::str(source)),
    ])
}

/// A compile cache over a content-addressed store.
///
/// The memo table lives in memory for the life of this struct; the store is
/// whatever `Cas` it was handed, so a `DirCas` makes compiled modules outlive
/// the process while a `MemoryCas` keeps them to it.
pub struct ModuleCache<'a, C: Cas + ?Sized> {
    cas: &'a C,
    table: MemoTable,
    /// Keyed rows pin nothing, so this is a scratch lock that stays empty. It
    /// exists because `PerformCtx` wants one.
    lock: EffectLock,
    config: KernelConfig,
    /// Where rows are written so the next process starts warm. Without it the
    /// objects still survive in a `DirCas` and nothing can find them, because
    /// what a new process lacks is not the bytes but the mapping from request
    /// to address.
    rows: Option<&'a DirRows>,
    /// Corruption found while reading the store, drained by the caller.
    ///
    /// Kept rather than printed because a library that picks its own logging
    /// sink is one the caller cannot quieten, and thrown away by nobody
    /// because a store that silently re-performs everything looks exactly
    /// like a cold one.
    corruption: Vec<String>,
    hits: u64,
    misses: u64,
}

/// What a cached compile did and produced.
pub struct Compiled {
    pub module: Rc<Module>,
    pub outcome: Outcome,
    pub id: ObjId,
}

/// Why a cached compile failed.
#[derive(Debug)]
pub enum CacheError {
    /// The compiler rejected the source. Carries the compiler's own error
    /// rather than its text, because the three kinds are three different
    /// exceptions to the embedder and the text cannot be classified back.
    Compile(crate::compile::CompileError),
    /// The store or the table failed.
    Kernel(KernelError),
    /// An object came back from the store and was not a module. This is
    /// corruption, not a miss: the address said what the bytes were.
    Corrupt {
        id: ObjId,
        detail: ModuleDecodeError,
    },
    /// The store does not have an object its own table points at.
    Dangling { id: ObjId },
}

impl core::fmt::Display for CacheError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Compile(detail) => write!(f, "{detail:?}"),
            Self::Kernel(source) => write!(f, "{source}"),
            Self::Corrupt { id, detail } => {
                write!(f, "object {id} is not a compiled module: {detail}")
            }
            Self::Dangling { id } => write!(f, "the compile cache points at absent object {id}"),
        }
    }
}

impl core::error::Error for CacheError {}

impl From<KernelError> for CacheError {
    fn from(source: KernelError) -> Self {
        Self::Kernel(source)
    }
}

impl<'a, C: Cas + ?Sized> ModuleCache<'a, C> {
    pub fn new(cas: &'a C) -> Self {
        Self {
            cas,
            table: MemoTable::new(),
            lock: EffectLock::new(),
            config: KernelConfig::default(),
            rows: None,
            corruption: Vec::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// Open a cache warmed from rows a previous process wrote.
    ///
    /// Returns the load report alongside, because a store whose rows were all
    /// refused behaves exactly like a cold one and the caller is the only
    /// place that can say so out loud.
    /// Open a cache backed by rows a previous process wrote.
    ///
    /// Nothing is read here. Rows are fetched one key at a time, when a
    /// request asks for one, because loading the domain up front costs every
    /// process O(everything anybody ever cached) to answer whatever it
    /// actually asked. Measured: the eager version made a warm store 3.4%
    /// slower than no store at all on a corpus of cheap files, which is the
    /// whole feature inverted.
    pub fn persistent(cas: &'a C, rows: &'a DirRows) -> Self {
        let mut cache = Self::new(cas);
        cache.rows = Some(rows);
        cache
    }

    /// Bring one key in from disk if this process has not seen it.
    ///
    /// A row naming an object the store does not have is dropped here rather
    /// than at use: objects and rows are swept independently, so a row can
    /// outlive what it points at.
    fn warm(&mut self, key: ix_kernel::Key) {
        let Some(rows) = self.rows else { return };
        if self.table.get(compile_domain(), key).is_some() {
            return;
        }
        match rows.get(compile_domain(), key) {
            Lookup::Missing => {}
            Lookup::Refused(reason) => self.corruption.push(reason.to_string()),
            Lookup::Found(output) => {
                if self.cas.has(output).unwrap_or(false) {
                    self.table.insert(
                        compile_domain(),
                        key,
                        ix_kernel::Entry {
                            output,
                            policy: Policy::Keyed,
                            provenance: ix_kernel::Provenance::Deterministic,
                        },
                    );
                } else {
                    self.corruption.push(format!(
                        "a compile row names object {output}, which the store does not have"
                    ));
                }
            }
        }
    }

    /// Take the corruption found since the last call. Empty is the normal
    /// case; anything here means a stored object was unusable and the work was
    /// redone.
    pub fn take_corruption(&mut self) -> Vec<String> {
        core::mem::take(&mut self.corruption)
    }

    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits
    }

    #[must_use]
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Compile, or return the module a previous compile of the same request
    /// produced.
    ///
    /// The decode happens on both paths, not just the hit path. On a miss the
    /// module is encoded, stored, and then read back and decoded rather than
    /// being returned directly, so a module that does not survive the round
    /// trip fails on the run that created it instead of on somebody's next
    /// run. Costing one decode per compile is worth never shipping a
    /// write-only encoder.
    pub fn compile(
        &mut self,
        source: &str,
        base_dir: &str,
        origin: crate::compile::Origin<'_>,
        settings: &crate::eval::Settings,
    ) -> Result<Compiled, CacheError> {
        let encoded_request = canon::encode(&request(base_dir, source, origin, settings))
            .map_err(KernelError::from)?;
        self.warm(ix_kernel::Key::mint(compile_domain(), &encoded_request));

        // The kernel's effect channel flattens a failure to text, and three
        // of this compiler's failures are three different exceptions on the
        // other side of the C ABI: a parse error, an undefined variable and
        // an unimplemented construct. Catching the error here on its way past
        // keeps the class, which going through the text would not: `nix eval`
        // with a cache directory used to report every one of them as a plain
        // evaluation error reading "effect in domain <64 hex> failed:
        // Parse(...)", while the same expression without a cache reported a
        // parse error with cppnix's own wording.
        let mut rejected: Option<crate::compile::CompileError> = None;
        let performed = on_perform(
            PerformCtx {
                table: &mut self.table,
                lock: &mut self.lock,
                cas: self.cas,
                config: &self.config,
                // Keyed rows carry Deterministic provenance and record no pin,
                // so neither of these reaches the table.
                performed_at: "",
                blessed_by: "",
            },
            compile_domain(),
            &Policy::Keyed,
            &encoded_request,
            || {
                crate::compile::compile_source(source, base_dir, origin, settings)
                    .map_err(|e| {
                        let text = format!("{e:?}");
                        rejected = Some(e);
                        text
                    })
                    .and_then(|module| {
                        encode_module(&module).map_err(|e| format!("cannot encode module: {e}"))
                    })
            },
        );
        let performed = match performed {
            Ok(performed) => performed,
            // Only a failure the compiler itself raised is a Compile error.
            // A store that broke while performing is still a Kernel error,
            // and `rejected` is what tells the two apart.
            Err(error) => {
                return Err(match rejected {
                    Some(compile_error) => CacheError::Compile(compile_error),
                    None => CacheError::Kernel(error),
                });
            }
        };

        match performed.outcome {
            Outcome::Hit => {
                self.hits += 1;
                // Mark the row used, so a sweep evicting by recency keeps the
                // working set rather than whatever was written most recently.
                if let Some(rows) = self.rows {
                    rows.touch(compile_domain(), performed.key);
                }
            }
            _ => {
                self.misses += 1;
                // Only a fresh row needs writing; a hit is already on disk, or
                // came from an in-memory table this process filled.
                if let Some(rows) = self.rows {
                    rows.put(compile_domain(), &encoded_request, performed.output)?;
                }
            }
        }

        match self.load_module(performed.output) {
            Ok(module) => Ok(Compiled {
                module: Rc::new(module),
                outcome: performed.outcome,
                id: performed.output,
            }),
            Err(detail) => {
                // An unusable object makes the row worthless, not the request
                // unanswerable. Keyed means re-performing is always safe, so
                // the row is dropped and the compile is redone rather than
                // failing a build over a damaged cache. The only way this can
                // recur is a compiler that does not round-trip, which the
                // second attempt would surface as a real error.
                self.corruption.push(format!(
                    "object {} for a compiled module was unusable ({detail}); recompiling",
                    performed.output
                ));
                // The row said hit and the object did not deliver, so this was
                // not one. Leaving the counter alone would report a cache that
                // recomputes everything as a cache that serves everything,
                // which is the one number anybody checks to see if it works.
                if performed.outcome == Outcome::Hit {
                    self.hits = self.hits.saturating_sub(1);
                    self.misses += 1;
                }
                self.table.remove(compile_domain(), performed.key);
                let module = crate::compile::compile_source(source, base_dir, origin, settings)
                    .map_err(CacheError::Compile)?;
                // A module that will not encode is this crate's bug, not the
                // user's source, so it keeps the compiler's own error kind
                // only for the source half above.
                let bytes = encode_module(&module).map_err(|e| {
                    CacheError::Compile(crate::compile::CompileError::Unimplemented(Refusal::new(
                        RefusalToken::UnsupportedOp,
                        format!("cannot encode module: {e}"),
                    )))
                })?;
                let id = self.cas.put(&bytes)?;
                Ok(Compiled {
                    module: Rc::new(module),
                    outcome: performed.outcome,
                    id,
                })
            }
        }
    }

    /// Read one object back as a module, refusing anything the address does
    /// not vouch for. `DirCas` names files by address but does not re-hash on
    /// read, so without this a truncated or swapped file would be decoded as
    /// though the address had checked it.
    fn load_module(&self, id: ObjId) -> Result<Module, String> {
        let bytes = self
            .cas
            .get(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "the store does not have it".to_owned())?;
        if ObjId::of(&bytes) != id {
            return Err("it does not hash to its address".to_owned());
        }
        decode_module(&bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile_source;
    use crate::eval::drive;
    use crate::host::RealFs;
    use crate::ir::OpKind;
    use crate::value2::Value;
    use crate::vm::Vm;
    use ix_kernel::MemoryCas;
    use std::collections::BTreeSet;

    /// One source for every op the compiler can emit, so the round-trip tests
    /// below cover the whole encoding rather than the easy half.
    ///
    /// That is a property of `compile.rs`, which has no idea this list exists,
    /// so `shapes_reach_every_op_the_compiler_emits` holds it rather than this
    /// comment. Adding a shape here is not what keeps it true; the guard
    /// failing is.
    const SHAPES: &[&str] = &[
        // A `rec` set that both overrides and names a dynamic attribute: the
        // only shape that reaches `MkAttrsOnto`, which exists so the dynamic
        // names land on the post-override set and keep cppnix's duplicate
        // check (`eval.cc:1489`).
        // `${k}` and not `${\"b\"}`: a string literal folds to a static name
        // in the parser (`parser-state.hh:91`), so the literal form compiles
        // to no dynamic attribute at all and would not reach the op.
        "let k = \"b\"; in rec { __overrides = { a = 2; }; a = 1; ${k} = 3; }",
        "1 + 2",
        "1.5 * 2.0",
        "true && false",
        "null",
        "\"abc\"",
        "let x = \"b\"; in \"a${x}c\"",
        "[ 1 2 3 ] ++ [ 4 ]",
        "{ a = 1; b = 2; } // { c = 3; }",
        "rec { a = 1; b = a + 1; }",
        "{ a = 1; }.a",
        "{ a = 1; }.b or 7",
        "{ a = 1; } ? a",
        "let s = \"a\"; in { a = 1; }.${s}",
        "let s = \"a\"; in { a = 1; } ? ${s}",
        "with { a = 1; }; a",
        "x: y: x + y",
        "{ a, b ? 2, ... } @ all: a + b",
        "if 1 < 2 then 3 else 4",
        "assert 1 == 1; 42",
        "builtins.length [ 1 2 ]",
        "builtins",
        "map (x: x + 1) [ 1 2 ]",
        "let f = n: if n == 0 then 1 else n * f (n - 1); in f 5",
        "-5",
        "!true",
        "1 != 2",
        "1 <= 2",
        // The four below were added by the coverage guard above, which found
        // that nothing in this corpus reached them.
        // The two globals that are neither primop nor constant, and so get an
        // op each rather than a table index.
        "derivation",
        "__nixPath",
        // A global cppnix registers and this evaluator has no entry for, which
        // compiles to a report rather than to a missing variable.
        "fetchMercurial",
        // Dynamic select with a default: the guarded sibling of the
        // `.${s}` shape above, which compiles to a different op.
        "let s = \"a\"; in { a = 1; }.${s} or 7",
        "2 >= 1",
        "2 > 1",
        "7 / 2",
        "toString 42",
        "let { body = a; a = 1; }",
        // An interpolated path literal, which is the only source of
        // `ConcatPath`: a plain path literal is one `Const`.
        "let v = \"x\"; in ./${v}/f.patch",
    ];

    /// `CompileError` does not implement `std::error::Error`, so it cannot be
    /// boxed by `?`; flattening it here keeps the tests' error type one thing.
    fn build(src: &str) -> Result<Module, String> {
        compile_source(
            src,
            "/base",
            crate::compile::Origin::String,
            &crate::eval::Settings::default(),
        )
        .map_err(|e| format!("{e:?}"))
    }

    fn evaluate(module: &Rc<Module>) -> String {
        let mut vm = Vm::with_settings(crate::eval::Settings::default());
        vm.start_module(module);
        let outcome = drive(&mut vm, &RealFs).and_then(|value| {
            vm.start_print(value);
            drive(&mut vm, &RealFs)
        });
        match outcome {
            Ok(Value::Str(s)) => s.expect_text(),
            other => format!("{other:?}"),
        }
    }

    /// Ops no Nix source can produce, with the reason each one cannot.
    ///
    /// An entry is a decision, not an oversight, and the test below checks it
    /// in both directions: an excluded op that becomes reachable fails just as
    /// loudly as a covered op that stops being covered. Otherwise the list
    /// would quietly outlive its reasons and turn the coverage number into
    /// whatever it happens to be.
    const UNREACHABLE_OPS: &[(&str, &str)] = &[(
        "CallBuiltin",
        "vestigial: `compile.rs` emits it nowhere and `vm.rs` answers it with          Unimplemented, so it exists only as an encoding tag",
    )];

    /// `SHAPES` claims to cover every op, and this is what makes the claim
    /// true rather than aspirational. It was false when written: five ops --
    /// the three globals, `SelectSoftDyn` and `CallBuiltin` -- had no round
    /// trip at all, so the encoding for them was never exercised by anything.
    #[test]
    fn shapes_reach_every_op_the_compiler_emits() -> Result<(), Box<dyn core::error::Error>> {
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        for src in SHAPES {
            let module = build(src)?;
            for unit in &module.units {
                for op in &unit.ops {
                    seen.insert(op.kind().name());
                }
            }
        }

        // A `build` that started handing back empty modules would satisfy
        // every "is not covered" check below by covering nothing at all.
        assert!(
            seen.len() > OpKind::COUNT / 2,
            "only {} ops seen across {} shapes; the corpus is not compiling",
            seen.len(),
            SHAPES.len()
        );

        let excluded: BTreeSet<&str> = UNREACHABLE_OPS.iter().map(|(op, _)| *op).collect();

        let uncovered: Vec<&str> = OpKind::ALL
            .iter()
            .map(|k| k.name())
            .filter(|name| !seen.contains(name) && !excluded.contains(name))
            .collect();
        assert!(
            uncovered.is_empty(),
            "no source in SHAPES compiles to {uncovered:?}, so the round-trip              tests never encode them; add a shape, or list the op in              UNREACHABLE_OPS with the reason it cannot be reached"
        );

        let reachable_after_all: Vec<&(&str, &str)> = UNREACHABLE_OPS
            .iter()
            .filter(|(op, _)| seen.contains(op))
            .collect();
        assert!(
            reachable_after_all.is_empty(),
            "listed as unreachable but SHAPES produces them: {reachable_after_all:?}              -- the reason has expired, drop the entry"
        );
        Ok(())
    }

    /// Two ops must not share an encoding tag, and every op must decode back
    /// to itself.
    ///
    /// `every_shape_round_trips_to_the_same_bytes` is blind to a collision,
    /// which is how this test came to exist: giving `ConcatPath` a tag
    /// `ConcatStrings` already held made an interpolated path encode as a
    /// string concatenation, decode as one, and re-encode to the *same
    /// bytes*, so the byte comparison passed while the module's meaning had
    /// changed. `rustc`'s unreachable-pattern warning catches the collision
    /// only when the decoder is edited too, and a warning is not a gate.
    ///
    /// Reading the tag off the encoded value rather than off the source is
    /// what makes this checkable at all: the tags are literals scattered
    /// through `op_value`, and reading them by eye is how the first
    /// collision happened -- a grep for the largest `unary(N` missed the
    /// `nullary` spellings and reported 42 when 44 was taken.
    #[test]
    fn every_op_has_its_own_tag_and_decodes_back_to_itself() {
        let mut by_tag: std::collections::BTreeMap<i128, &'static str> =
            std::collections::BTreeMap::new();
        for op in &crate::ir::one_of_each() {
            let encoded = op_value(op);
            let CanonValue::Array(parts) = &encoded else {
                unreachable!("an op encodes as an array; got {encoded:?}")
            };
            let Some(CanonValue::Int(tag)) = parts.first() else {
                unreachable!("an op's first field is its tag; got {encoded:?}")
            };
            if let Some(other) = by_tag.insert(*tag, op.kind().name()) {
                unreachable!(
                    "tag {tag} is used by both {other} and {}; a module encoded \
                     with one decodes as the other",
                    op.kind().name()
                );
            }
            let back = op_from(&encoded);
            assert!(
                matches!(&back, Ok(b) if b == op),
                "{} encoded to tag {tag} and decoded back as {back:?}",
                op.kind().name()
            );
        }
        assert_eq!(by_tag.len(), OpKind::COUNT, "an op was not sampled");
    }

    #[test]
    fn every_shape_round_trips_to_the_same_bytes() -> Result<(), Box<dyn core::error::Error>> {
        for src in SHAPES {
            let module = build(src)?;
            let bytes = encode_module(&module)?;
            let back = decode_module(&bytes)?;
            assert_eq!(encode_module(&back)?, bytes, "source {src}");
        }
        Ok(())
    }

    /// The property M-A exists to establish: a module that has been through
    /// the store evaluates to the same thing as one that has not.
    #[test]
    fn a_decoded_module_evaluates_identically() -> Result<(), Box<dyn core::error::Error>> {
        for src in SHAPES {
            let fresh = Rc::new(build(src)?);
            let decoded = Rc::new(decode_module(&encode_module(&fresh)?)?);
            assert_eq!(evaluate(&fresh), evaluate(&decoded), "source {src}");
        }
        Ok(())
    }

    #[test]
    fn the_second_compile_of_one_source_is_a_hit() -> Result<(), Box<dyn core::error::Error>> {
        let cas = MemoryCas::new();
        let mut cache = ModuleCache::new(&cas);
        let first = cache.compile(
            "1 + 2",
            "/base",
            crate::compile::Origin::String,
            &crate::eval::Settings::default(),
        )?;
        let second = cache.compile(
            "1 + 2",
            "/base",
            crate::compile::Origin::String,
            &crate::eval::Settings::default(),
        )?;
        assert_eq!(first.outcome, Outcome::Performed);
        assert_eq!(second.outcome, Outcome::Hit);
        assert_eq!(first.id, second.id);
        assert_eq!(evaluate(&second.module), "3");
        assert_eq!((cache.hits(), cache.misses()), (1, 1));
        Ok(())
    }

    /// The base directory is part of the key because it is part of the
    /// module: path literals are made absolute against it.
    #[test]
    fn the_same_source_under_two_base_dirs_does_not_share_a_row()
    -> Result<(), Box<dyn core::error::Error>> {
        let cas = MemoryCas::new();
        let mut cache = ModuleCache::new(&cas);
        let here = cache.compile(
            "./x.nix",
            "/one",
            crate::compile::Origin::String,
            &crate::eval::Settings::default(),
        )?;
        let there = cache.compile(
            "./x.nix",
            "/two",
            crate::compile::Origin::String,
            &crate::eval::Settings::default(),
        )?;
        assert_eq!(here.outcome, Outcome::Performed);
        assert_eq!(
            there.outcome,
            Outcome::Performed,
            "second base dir hit the first's row"
        );
        assert_ne!(here.id, there.id);
        Ok(())
    }

    /// The origin is part of the key for the same reason and a stronger one:
    /// `__curPos` compiles to the name of the file the token is written in,
    /// so two files with identical text in the same directory are two
    /// modules. Keying on the base directory alone would serve the first
    /// file's name for the second, and that name reaches derivations through
    /// nixpkgs' `meta.position`.
    #[test]
    fn the_same_source_at_two_paths_does_not_share_a_row() -> Result<(), Box<dyn core::error::Error>>
    {
        let cas = MemoryCas::new();
        let mut cache = ModuleCache::new(&cas);
        let source = "__curPos";
        let one = cache.compile(
            source,
            "/dir",
            crate::compile::Origin::File("/dir/one.nix"),
            &crate::eval::Settings::default(),
        )?;
        let two = cache.compile(
            source,
            "/dir",
            crate::compile::Origin::File("/dir/two.nix"),
            &crate::eval::Settings::default(),
        )?;
        assert_eq!(one.outcome, Outcome::Performed);
        assert_eq!(
            two.outcome,
            Outcome::Performed,
            "the second path hit the first's row"
        );
        assert_ne!(one.id, two.id);
        // And the rendered answers differ, which is the consequence the key
        // exists to prevent -- two ids over one wrong constant would satisfy
        // the assertion above.
        assert_ne!(evaluate(&one.module), evaluate(&two.module));
        assert!(
            evaluate(&one.module).contains("/dir/one.nix"),
            "{}",
            evaluate(&one.module)
        );
        assert!(
            evaluate(&two.module).contains("/dir/two.nix"),
            "{}",
            evaluate(&two.module)
        );
        // The same path twice is still one row.
        let again = cache.compile(
            source,
            "/dir",
            crate::compile::Origin::File("/dir/one.nix"),
            &crate::eval::Settings::default(),
        )?;
        assert_eq!(again.outcome, Outcome::Hit);
        Ok(())
    }

    /// Two sources that differ produce two rows; this is the check that the
    /// key is not accidentally constant.
    #[test]
    fn different_sources_do_not_share_a_row() -> Result<(), Box<dyn core::error::Error>> {
        let cas = MemoryCas::new();
        let mut cache = ModuleCache::new(&cas);
        assert_ne!(
            cache
                .compile(
                    "1 + 2",
                    "/base",
                    crate::compile::Origin::String,
                    &crate::eval::Settings::default()
                )?
                .id,
            cache
                .compile(
                    "1 + 3",
                    "/base",
                    crate::compile::Origin::String,
                    &crate::eval::Settings::default()
                )?
                .id
        );
        assert_eq!(cache.misses(), 2);
        Ok(())
    }

    /// Float constants go through as IEEE bits, so the ones a decimal
    /// rendering would round or normalise have to survive exactly.
    #[test]
    fn awkward_floats_survive_the_encoding() -> Result<(), Box<dyn core::error::Error>> {
        for src in ["1.0e308 + 0.0", "0.1 + 0.0", "-0.0 + 0.0", "1.0e-320 + 0.0"] {
            let fresh = Rc::new(build(src)?);
            let decoded = Rc::new(decode_module(&encode_module(&fresh)?)?);
            assert_eq!(evaluate(&fresh), evaluate(&decoded), "source {src}");
        }
        Ok(())
    }

    #[test]
    fn a_truncated_object_is_refused_rather_than_half_decoded() {
        let Ok(module) = build("1 + 2") else {
            return;
        };
        let Ok(bytes) = encode_module(&module) else {
            return;
        };
        let cut = bytes.get(..bytes.len() - 1).unwrap_or(&[]);
        assert!(matches!(
            decode_module(cut),
            Err(ModuleDecodeError::Canon(_))
        ));
    }

    #[test]
    fn an_entry_naming_no_unit_is_refused() -> Result<(), Box<dyn core::error::Error>> {
        let value = CanonValue::map([
            ("consts", CanonValue::Array(Vec::new())),
            ("symbols", CanonValue::Array(Vec::new())),
            ("units", CanonValue::Array(Vec::new())),
            ("entry", CanonValue::int(0u32)),
        ]);
        assert!(matches!(
            decode_module(&canon::encode(&value)?),
            Err(ModuleDecodeError::Shape(_))
        ));
        Ok(())
    }

    /// The fingerprint is what stops a module compiled by a different builtin
    /// table, or by a compiler that folds `builtins.<name>` differently, from
    /// being served out of a store an earlier build wrote.
    #[test]
    fn the_compiler_fingerprint_is_a_hash() {
        let fingerprint = compiler_fingerprint();
        assert_eq!(fingerprint.len(), 64, "fingerprint: {fingerprint}");
        assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    }

    include!("../compiler-fingerprint.rs");

    fn crate_root() -> &'static std::path::Path {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    /// The feature set this build was compiled with, as `build.rs` stamped it.
    /// Checked against the compiler's own answer by
    /// `the_stamped_feature_list_is_this_builds_own`.
    fn stamped_features() -> Vec<String> {
        env!("IXE_COMPILER_FEATURES")
            .split(',')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn fingerprint_inputs() -> Vec<FingerprintInput> {
        match compiler_fingerprint_inputs(crate_root(), &stamped_features()) {
            Ok(inputs) => inputs,
            Err(error) => unreachable!("the crate's own tree must be readable: {error}"),
        }
    }

    /// The stamped fingerprint really is a hash of this crate's inputs, and
    /// not a constant that survives an edit to them.
    ///
    /// Recomputed through the same two functions the build script calls,
    /// `include!`d rather than reimplemented, so this is a check on the build
    /// script and not a second implementation to keep in step.
    ///
    /// On its own it is a weak check, and was: it agrees with the build script
    /// whatever the two of them leave out, which is how `Cargo.lock` and the
    /// feature set stayed outside the hash. The two tests below it are the
    /// ones that can see an omission.
    #[test]
    fn the_fingerprint_is_recomputable_from_the_sources_it_claims_to_cover() {
        let recomputed = hash_compiler_fingerprint_inputs(&fingerprint_inputs());
        assert_eq!(
            recomputed.as_deref(),
            Ok(compiler_fingerprint()),
            "the stamped fingerprint does not describe {}",
            crate_root().display()
        );
    }

    /// The input set names every file that decides what a compiled `Module`
    /// means, and every file the binary embeds is one of them.
    ///
    /// This is the test the recompute-and-compare one structurally cannot be:
    /// it reads the *names* rather than the hash, so an input that was never
    /// collected is visible here and invisible there.
    ///
    /// Two halves, and they check different directions. The first walks the
    /// tree and requires everything it finds to be in the input set -- the
    /// walk is written out again on purpose, iteratively where the shared code
    /// recurses, because calling `collect_rust_sources` would make it agree
    /// with the thing it is checking. The second scans the sources for
    /// `include_str!` and `include_bytes!` and requires each target to be
    /// either a `.rs` the walk already covers or an entry in
    /// `EMBEDDED_FILES` -- and then requires each `EMBEDDED_FILES` entry to
    /// actually reach the hash, which "the table is complete" does not imply.
    ///
    /// Both halves were live bugs, one each: `Cargo.lock` and the feature set
    /// were outside the hash, and so was `derivation.nix`, which `vm.rs`
    /// `include_str!`s from three levels above the crate and which is the body
    /// of the `derivation` global -- so its text reaches every derivation an
    /// expression builds, and editing it moved neither the compile-cache
    /// request nor `EvalId` (ENG-13010).
    #[test]
    fn the_fingerprint_input_set_names_everything_that_can_change_the_compiler() {
        let inputs = fingerprint_inputs();
        let names: Vec<&str> = inputs.iter().map(|input| input.name.as_str()).collect();

        for required in ["Cargo.lock", "Cargo.toml", "features"] {
            assert!(
                names.contains(&required),
                "{required} is not in the fingerprint's input set: {names:?}"
            );
        }
        // Every declared embed reaches the hash. Their presence in the table
        // is what the scan below checks; that the table is *used* is this.
        for (name, _) in EMBEDDED_FILES {
            assert!(
                names.iter().any(|listed| listed == name),
                "{name} is declared in EMBEDDED_FILES and is not in the input set, so the \
                 table is decorative: {names:?}"
            );
        }

        // An independent walk of `src`, iterative where the shared one
        // recurses, so a bug in one is not a bug in both. Recursive, because
        // the source scan below has to see a module that moved into a
        // subdirectory too.
        let src = crate_root().join("src");
        let mut todo = vec![src.clone()];
        let mut sources = Vec::new();
        while let Some(dir) = todo.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                unreachable!("cannot list {}", dir.display())
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    todo.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    sources.push(path);
                }
            }
        }
        assert!(
            sources.len() > 1,
            "the walk found {} source files, so it found nothing",
            sources.len()
        );
        for path in &sources {
            let Ok(relative) = path.strip_prefix(&src) else {
                unreachable!("{} is not under {}", path.display(), src.display())
            };
            let expected = format!("src/{}", relative.to_string_lossy());
            assert!(
                names.iter().any(|name| *name == expected),
                "{expected} is not in the fingerprint's input set: {names:?}"
            );
        }

        // The source scan: anything the binary embeds must be covered.
        let listed: Vec<&str> = EMBEDDED_FILES.iter().map(|(_, rel)| *rel).collect();
        let mut checked = 0_usize;
        for path in &sources {
            let Ok(text) = std::fs::read_to_string(path) else {
                unreachable!("cannot read {}", path.display())
            };
            for (macro_name, rest) in text
                .match_indices("include_str!")
                .chain(text.match_indices("include_bytes!"))
                .map(|(i, m)| (m, text.get(i + m.len()..).unwrap_or_default()))
            {
                // An invocation, not a mention: the macro name has to be
                // followed by its paren. Without this the scan matches the
                // word in its own doc comment and then reads whatever string
                // comes next, which is how the first version of this reported
                // `CARGO_MANIFEST_DIR` as an embedded file.
                if !rest.trim_start().starts_with('(') {
                    continue;
                }
                // The macro's first argument: what lies between the first pair
                // of quotes after the paren.
                let Some(open) = rest.find('"') else { continue };
                let Some(len) = rest.get(open + 1..).and_then(|r| r.find('"')) else {
                    continue;
                };
                let Some(target) = rest.get(open + 1..open + 1 + len) else {
                    continue;
                };
                checked += 1;

                // A sibling `.rs` is already covered by the walk above.
                if !target.contains('/') && target.ends_with(".rs") {
                    continue;
                }
                // Anything else has to be declared. Compared on the tail so
                // the crate-root-relative spelling in `EMBEDDED_FILES` and the
                // `src/`-relative spelling at the call site can differ by the
                // one leading `../` that separates them.
                let covered = listed
                    .iter()
                    .any(|rel| rel.trim_start_matches("../") == target.trim_start_matches("../"));
                assert!(
                    covered,
                    "{} embeds {target:?} via {macro_name} and it is not in EMBEDDED_FILES, so \
                     editing it will not move the compiler fingerprint and a store written \
                     before the edit will answer after it",
                    path.display(),
                );
            }
        }
        assert!(
            checked >= 4,
            "the scan found only {checked} embedded files; it has stopped reading the sources \
             and would pass with the fingerprint wide open"
        );
    }

    /// Changing any one input changes the fingerprint, and so does renaming
    /// one.
    ///
    /// The rename half is not decoration: without the length prefix on the
    /// name, moving a byte from one input's name into the next one's would
    /// leave the hash where it was.
    #[test]
    fn changing_any_fingerprint_input_changes_the_fingerprint() {
        let inputs = fingerprint_inputs();
        let Ok(base) = hash_compiler_fingerprint_inputs(&inputs) else {
            unreachable!("hashing a set that was just built must succeed")
        };

        for index in 0..inputs.len() {
            let mut mutated: Vec<FingerprintInput> = inputs
                .iter()
                .map(|input| FingerprintInput {
                    name: input.name.clone(),
                    path: input.path.clone(),
                    bytes: input.bytes.clone(),
                })
                .collect();
            let Some(entry) = mutated.get_mut(index) else {
                unreachable!("index {index} is inside a vector of {}", inputs.len())
            };
            entry.bytes.push(b'\n');
            let name = entry.name.clone();
            let Ok(moved) = hash_compiler_fingerprint_inputs(&mutated) else {
                unreachable!("hashing a mutated set must succeed")
            };
            assert_ne!(
                base, moved,
                "editing {name} left the fingerprint where it was"
            );
        }
    }

    /// `build.rs` reads the feature set from the environment; the compiler
    /// answers `cfg!`. This is where the two are made to agree, so the
    /// recompute test is not the build script checking its own claim.
    #[test]
    fn the_stamped_feature_list_is_this_builds_own() {
        // Named the way cargo names them in `CARGO_FEATURE_*`.
        let declared = [
            ("DEFAULT", cfg!(feature = "default")),
            ("PERF", cfg!(feature = "perf")),
            ("PERF_OPS", cfg!(feature = "perf-ops")),
        ];

        // The table above has to be the whole of `[features]`, or a feature
        // added later is one this test silently stops checking.
        let manifest = crate_root().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            unreachable!("cannot read {}", manifest.display())
        };
        // `toml::from_str` and not `text.parse::<toml::Value>()`: the latter
        // parses a single value, and a manifest is a document.
        let parsed: toml::Value = match toml::from_str(&text) {
            Ok(parsed) => parsed,
            Err(error) => unreachable!("cannot parse {}: {error}", manifest.display()),
        };
        let Some(table) = parsed.get("features").and_then(toml::Value::as_table) else {
            unreachable!("{} has no [features] table", manifest.display())
        };
        let mut from_manifest: Vec<String> = table
            .keys()
            .map(|key| key.to_uppercase().replace('-', "_"))
            .collect();
        from_manifest.sort();
        let mut from_table: Vec<String> = declared
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        from_table.sort();
        assert_eq!(
            from_manifest, from_table,
            "this test's feature table is not the crate's [features] table"
        );

        let mut expected: Vec<String> = declared
            .iter()
            .filter(|(_, on)| *on)
            .map(|(name, _)| (*name).to_owned())
            .collect();
        expected.sort();
        let mut stamped = stamped_features();
        stamped.sort();
        assert_eq!(
            stamped, expected,
            "build.rs stamped a feature set the compiler disagrees with"
        );
    }

    // ---- persistence -----------------------------------------------------

    use ix_kernel::cas::{Cas, DirCas};
    use ix_kernel::rows::DirRows;
    use std::path::PathBuf;

    fn scratch(label: &str) -> PathBuf {
        crate::eval::scratch_dir("ixe-modcache", label)
    }

    /// The M-D property for compilation: a cache built fresh over a store a
    /// previous one wrote does not recompile.
    #[test]
    fn a_new_cache_over_a_warm_store_hits() -> Result<(), Box<dyn core::error::Error>> {
        let dir = scratch("warm");
        let result = (|| -> Result<(), Box<dyn core::error::Error>> {
            let cas = DirCas::open(dir.join("objects"))?;
            let rows = DirRows::open(dir.join("index"))?;

            let first = {
                let mut cache = ModuleCache::persistent(&cas, &rows);
                let compiled = cache.compile(
                    "1 + 2",
                    "/base",
                    crate::compile::Origin::String,
                    &crate::eval::Settings::default(),
                )?;
                assert_eq!(compiled.outcome, Outcome::Performed);
                compiled.id
            };

            // Everything from the first cache is dropped here; only the
            // directories survive, which is what a second process sees.
            let mut cache = ModuleCache::persistent(&cas, &rows);
            let second = cache.compile(
                "1 + 2",
                "/base",
                crate::compile::Origin::String,
                &crate::eval::Settings::default(),
            )?;
            assert_eq!(second.outcome, Outcome::Hit, "a warm store did not hit");
            assert_eq!(second.id, first);
            assert_eq!(evaluate(&second.module), "3");
            assert!(cache.take_corruption().is_empty());
            Ok(())
        })();
        drop(std::fs::remove_dir_all(&dir));
        result
    }

    /// Editing the source between the two caches must miss, exactly as it does
    /// within one process: the key is the content.
    #[test]
    fn a_new_cache_misses_when_the_source_changed() -> Result<(), Box<dyn core::error::Error>> {
        let dir = scratch("edited");
        let result = (|| -> Result<(), Box<dyn core::error::Error>> {
            let cas = DirCas::open(dir.join("objects"))?;
            let rows = DirRows::open(dir.join("index"))?;
            ModuleCache::persistent(&cas, &rows).compile(
                "1 + 2",
                "/base",
                crate::compile::Origin::String,
                &crate::eval::Settings::default(),
            )?;

            let mut cache = ModuleCache::persistent(&cas, &rows);
            let changed = cache.compile(
                "1 + 3",
                "/base",
                crate::compile::Origin::String,
                &crate::eval::Settings::default(),
            )?;
            assert_eq!(changed.outcome, Outcome::Performed);
            assert_eq!(evaluate(&changed.module), "4");
            Ok(())
        })();
        drop(std::fs::remove_dir_all(&dir));
        result
    }

    /// A damaged object is a miss with a reason, never an error and never a
    /// wrong module. Keyed means recompiling is always safe.
    #[test]
    fn a_truncated_object_is_recompiled_and_reported() -> Result<(), Box<dyn core::error::Error>> {
        let dir = scratch("truncated");
        let result = (|| -> Result<(), Box<dyn core::error::Error>> {
            let cas = DirCas::open(dir.join("objects"))?;
            let rows = DirRows::open(dir.join("index"))?;
            let id = ModuleCache::persistent(&cas, &rows)
                .compile(
                    "1 + 2",
                    "/base",
                    crate::compile::Origin::String,
                    &crate::eval::Settings::default(),
                )?
                .id;

            // Truncate the object in place, leaving the row pointing at it.
            let path = dir.join("objects").join(id.hash().to_hex());
            let bytes = std::fs::read(&path)?;
            std::fs::write(&path, bytes.get(..2).unwrap_or(&[]))?;

            let mut cache = ModuleCache::persistent(&cas, &rows);
            let compiled = cache.compile(
                "1 + 2",
                "/base",
                crate::compile::Origin::String,
                &crate::eval::Settings::default(),
            )?;
            assert_eq!(evaluate(&compiled.module), "3", "served a wrong module");
            let reported = cache.take_corruption();
            assert_eq!(
                reported.len(),
                1,
                "corruption went unreported: {reported:?}"
            );
            assert!(
                reported
                    .first()
                    .is_some_and(|r| r.contains("does not hash to its address")),
                "{reported:?}"
            );
            // And it counted as a miss, not the hit the row claimed.
            assert_eq!((cache.hits(), cache.misses()), (0, 1));
            Ok(())
        })();
        drop(std::fs::remove_dir_all(&dir));
        result
    }

    /// An object swapped for another *valid* object is the case a decode
    /// cannot catch: both parse. Only re-hashing against the address the row
    /// asked for separates them.
    #[test]
    fn a_swapped_but_valid_object_is_refused() -> Result<(), Box<dyn core::error::Error>> {
        let dir = scratch("swapped");
        let result = (|| -> Result<(), Box<dyn core::error::Error>> {
            let cas = DirCas::open(dir.join("objects"))?;
            let rows = DirRows::open(dir.join("index"))?;
            let (one, other) = {
                let mut cache = ModuleCache::persistent(&cas, &rows);
                (
                    cache
                        .compile(
                            "1 + 2",
                            "/base",
                            crate::compile::Origin::String,
                            &crate::eval::Settings::default(),
                        )?
                        .id,
                    cache
                        .compile(
                            "10 + 20",
                            "/base",
                            crate::compile::Origin::String,
                            &crate::eval::Settings::default(),
                        )?
                        .id,
                )
            };
            // Give one address the other's perfectly well formed bytes.
            let other_bytes = std::fs::read(dir.join("objects").join(other.hash().to_hex()))?;
            std::fs::write(dir.join("objects").join(one.hash().to_hex()), &other_bytes)?;

            let mut cache = ModuleCache::persistent(&cas, &rows);
            let compiled = cache.compile(
                "1 + 2",
                "/base",
                crate::compile::Origin::String,
                &crate::eval::Settings::default(),
            )?;
            assert_eq!(evaluate(&compiled.module), "3", "served the other module");
            assert_eq!(cache.take_corruption().len(), 1);
            Ok(())
        })();
        drop(std::fs::remove_dir_all(&dir));
        result
    }

    /// A row whose object was swept is dropped, with a reason, and the work
    /// is redone.
    #[test]
    fn a_row_whose_object_was_swept_is_refused() -> Result<(), Box<dyn core::error::Error>> {
        let dir = scratch("swept");
        let result = (|| -> Result<(), Box<dyn core::error::Error>> {
            let cas = DirCas::open(dir.join("objects"))?;
            let rows = DirRows::open(dir.join("index"))?;
            let id = ModuleCache::persistent(&cas, &rows)
                .compile(
                    "1 + 2",
                    "/base",
                    crate::compile::Origin::String,
                    &crate::eval::Settings::default(),
                )?
                .id;
            std::fs::remove_file(dir.join("objects").join(id.hash().to_hex()))?;

            let mut cache = ModuleCache::persistent(&cas, &rows);
            let compiled = cache.compile(
                "1 + 2",
                "/base",
                crate::compile::Origin::String,
                &crate::eval::Settings::default(),
            )?;
            assert_eq!(compiled.outcome, Outcome::Performed);
            assert_eq!(evaluate(&compiled.module), "3");
            let reported = cache.take_corruption();
            assert!(
                reported
                    .first()
                    .is_some_and(|r| r.contains("does not have")),
                "{reported:?}"
            );
            Ok(())
        })();
        drop(std::fs::remove_dir_all(&dir));
        result
    }

    /// A row filed under the wrong key is refused at lookup, so a mis-filed
    /// row cannot answer a request it was not computed for.
    #[test]
    fn a_misfiled_row_is_refused_at_lookup() -> Result<(), Box<dyn core::error::Error>> {
        let dir = scratch("misfiled");
        let result = (|| -> Result<(), Box<dyn core::error::Error>> {
            let cas = DirCas::open(dir.join("objects"))?;
            let rows = DirRows::open(dir.join("index"))?;
            ModuleCache::persistent(&cas, &rows).compile(
                "1 + 2",
                "/base",
                crate::compile::Origin::String,
                &crate::eval::Settings::default(),
            )?;

            // Rename the one row to some other key in the same domain.
            let domain_dir = dir.join("index").join(compile_domain().hash().to_hex());
            let only = std::fs::read_dir(&domain_dir)?
                .filter_map(|e| Some(e.ok()?.path()))
                .find(|p| p.is_file())
                .ok_or("no row written")?;
            let target = domain_dir.join("0".repeat(64));
            std::fs::rename(&only, &target)?;

            // Ask for the key the row now claims to be. It must be refused.
            let mut cache = ModuleCache::persistent(&cas, &rows);
            cache.warm(ix_kernel::Key::from_hash(ix_kernel::Hash::from_hex(
                &"0".repeat(64),
            )?));
            let reported = cache.take_corruption();
            assert!(
                reported.first().is_some_and(|r| r.contains("wrong key")),
                "{reported:?}"
            );
            Ok(())
        })();
        drop(std::fs::remove_dir_all(&dir));
        result
    }

    /// Objects written by one cache are addressable by the other, which is
    /// the half of persistence the CAS already provided; this pins it.
    #[test]
    fn objects_outlive_the_cache_that_wrote_them() -> Result<(), Box<dyn core::error::Error>> {
        let dir = scratch("objects");
        let result = (|| -> Result<(), Box<dyn core::error::Error>> {
            let id = {
                let cas = DirCas::open(dir.join("objects"))?;
                let rows = DirRows::open(dir.join("index"))?;
                ModuleCache::persistent(&cas, &rows)
                    .compile(
                        "1 + 2",
                        "/base",
                        crate::compile::Origin::String,
                        &crate::eval::Settings::default(),
                    )?
                    .id
            };
            let cas = DirCas::open(dir.join("objects"))?;
            assert!(cas.has(id)?);
            Ok(())
        })();
        drop(std::fs::remove_dir_all(&dir));
        result
    }
}
