//! Runtime values for the VM. `Value` is deliberately cheap to clone: every
//! aggregate is behind `Rc`. The two-word packed representation from the
//! architecture plan replaces this once semantics are complete; behavior
//! first, representation second, measured by the corpus differ throughout.

use crate::ir::Module;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;

/// Interned symbol id within one VM instance.
pub type Sym = u32;

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    /// String with its (rarely present) context.
    Str(NixStr),
    Path(Rc<str>),
    List(Rc<Vec<Slot>>),
    /// Sorted by symbol id at construction; iteration order for printing is
    /// name-alphabetical, resolved through the interner at print time.
    Attrs(Rc<Attrs>),
    Closure(Rc<ClosureData>),
    /// A builtin, possibly partially applied (arity > args.len()).
    Builtin(Rc<BuiltinData>),
}

/// An attribute set: the bindings, and where they were written.
///
/// A wrapper around the map rather than a second field on `Value::Attrs`,
/// because `Value` is stored inline in every stack entry and every slot:
/// widening the enum by two words to carry a fact only
/// `builtins.unsafeGetAttrPos` reads would cost every value in the program.
/// On the heap beside the map it costs one `Rc` bump and 16 bytes per set.
///
/// `Deref` to the map, so the hundred-odd places that only want to look
/// something up read exactly as they did.
#[derive(Debug, Clone, Default)]
pub struct Attrs {
    map: BTreeMap<Sym, Slot>,
    /// Which `MkAttrs` built this set, or `None` when nothing in the source
    /// did.
    ///
    /// `None` is the honest answer for a set no source expression wrote --
    /// `builtins.listToAttrs`, `//`, a set built by the bridge -- and
    /// `unsafeGetAttrPos` answers `null` for those, which is what cppnix
    /// answers for an attribute with no recorded position. See
    /// [`AttrOrigin`] for why `//` is in that list.
    pub origin: Option<AttrOrigin>,
}

/// Which instruction of which unit built an attribute set.
///
/// Three words and no work: the names and their offsets are already in the
/// module's `attr_sites`, so construction copies a refcount and two integers
/// and the search only happens if someone calls `unsafeGetAttrPos`.
///
/// # What this cannot express, and why that is `null` rather than a guess
///
/// One set, one origin. cppnix stores a position per *attribute*, inside the
/// `Bindings`, so `a // b` keeps a's positions for a's attributes and b's for
/// b's, and `listToAttrs` gives each attribute the position of the element it
/// came from. A single origin cannot say that.
///
/// The rule that makes one origin safe anyway: **a derived set takes the
/// origin of the operand whose values it takes.** `//` takes the right's,
/// `removeAttrs` keeps its own, `intersectAttrs` takes the second's. Because
/// [`AttrOrigin::offset_of`] answers only for names that origin's `MkAttrs`
/// actually built, an attribute that came from somewhere else falls out as
/// `None` rather than as the wrong line. So the answer is cppnix's wherever
/// there is one, and `null` where there is not -- never a real line of a real
/// file belonging to a different attribute, which a reader could not tell
/// apart from a right answer.
///
/// What that costs, both pinned by tests in `tests/positions.rs`: an
/// attribute only the LEFT operand of `//` had answers `null` where cppnix
/// gives the left's position, and every attribute of a `listToAttrs` set
/// answers `null`. `maintainers/ix/positions.md` is the full statement.
#[derive(Debug, Clone)]
pub struct AttrOrigin {
    pub module: Rc<Module>,
    pub unit: u32,
    /// Index of the `MkAttrs` in the unit's ops, or [`AttrOrigin::FORMALS`].
    pub ip: u32,
}

impl AttrOrigin {
    /// `ip` for the set `builtins.functionArgs` builds out of a lambda's
    /// formal parameters, whose positions are on the `Param` and not on any
    /// instruction.
    pub const FORMALS: u32 = u32::MAX;

    /// Where the attribute named `name` was written, or `None` when this
    /// origin does not name it -- a dynamic attribute, or a name that came
    /// from somewhere else.
    #[must_use]
    pub fn offset_of(&self, name: &str) -> Option<u32> {
        let unit = self.module.units.get(self.unit as usize)?;
        if self.ip == AttrOrigin::FORMALS {
            let Some(crate::ir::Param::Formals { fields, .. }) = &unit.param else {
                return None;
            };
            return fields
                .iter()
                .find(|f| {
                    self.module
                        .symbols
                        .get(f.sym as usize)
                        .is_some_and(|s| s == name)
                })
                .map(|f| f.pos)
                .filter(|p| *p != crate::ir::NO_POS);
        }
        let site = unit
            .attr_sites
            .binary_search_by_key(&self.ip, |s| s.ip)
            .ok()
            .and_then(|i| unit.attr_sites.get(i))?;
        // `names` is sorted by the symbol's TEXT, which is what a lookup
        // arrives with; the symbol index is assignment order and no use here.
        site.names
            .binary_search_by(|(sym, _)| {
                self.module
                    .symbols
                    .get(*sym as usize)
                    .map_or("", String::as_str)
                    .cmp(name)
            })
            .ok()
            .and_then(|i| site.names.get(i))
            .map(|(_, offset)| *offset)
            .filter(|p| *p != crate::ir::NO_POS)
    }
}

impl Attrs {
    /// A set with no source behind it.
    #[must_use]
    pub fn new(map: BTreeMap<Sym, Slot>) -> Attrs {
        Attrs { map, origin: None }
    }

    /// A set with no source behind it, built from pairs already in strictly
    /// ascending `Sym` order.
    ///
    /// Exists so that a caller which produced its pairs in order -- a merge
    /// over two sets that are themselves sorted, like `intersectAttrs` --
    /// does not pay for a per-call `BTreeMap` rebuild through the tree's
    /// comparison path, and so that the order guarantee is stated at the
    /// construction site rather than rediscovered. It is also the
    /// constructor a future persistent (structurally shared) representation
    /// needs, where "build from a sorted stream" is the cheap bulk operation
    /// (ENG-13148, ENG-13152).
    ///
    /// Sortedness is the caller's contract, checked in debug builds.
    #[must_use]
    pub fn from_sorted_iter(iter: impl IntoIterator<Item = (Sym, Slot)>) -> Attrs {
        let mut prev: Option<Sym> = None;
        let map: BTreeMap<Sym, Slot> = iter
            .into_iter()
            .inspect(|(k, _)| {
                debug_assert!(
                    prev.is_none_or(|p| p < *k),
                    "from_sorted_iter: keys not in strictly ascending order"
                );
                prev = Some(*k);
            })
            .collect();
        Attrs { map, origin: None }
    }

    /// A set the given instruction built.
    #[must_use]
    pub fn at(map: BTreeMap<Sym, Slot>, origin: AttrOrigin) -> Attrs {
        Attrs {
            map,
            origin: Some(origin),
        }
    }

    /// The bindings, for the few callers that take the map apart.
    #[must_use]
    pub fn into_map(self) -> BTreeMap<Sym, Slot> {
        self.map
    }
}

impl From<BTreeMap<Sym, Slot>> for Attrs {
    fn from(map: BTreeMap<Sym, Slot>) -> Attrs {
        Attrs::new(map)
    }
}

impl std::ops::Deref for Attrs {
    type Target = BTreeMap<Sym, Slot>;

    fn deref(&self) -> &BTreeMap<Sym, Slot> {
        &self.map
    }
}

impl std::ops::DerefMut for Attrs {
    /// Mutating the bindings does NOT clear the origin, and the callers that
    /// rely on that are the derived sets: `//` and `removeAttrs` mutate a
    /// clone whose origin is deliberately the one whose values survive. See
    /// [`AttrOrigin`] for why that is safe, and `Attrs::new` for a set with
    /// no origin at all.
    fn deref_mut(&mut self) -> &mut BTreeMap<Sym, Slot> {
        &mut self.map
    }
}

/// One element of a string's context: what the string depends on.
///
/// cppnix's `NixStringContextElem`, with its three cases and their rendered
/// spellings, which are corpus-visible through `builtins.getContext` and are
/// what `derivationStrict` reads `inputSrcs` and `inputDrvs` out of.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextElem {
    /// A plain store path the string depends on: `/nix/store/...`.
    Opaque(Rc<str>),
    /// Every output of a derivation: `=/nix/store/....drv`.
    DrvDeep(Rc<str>),
    /// One named output of a derivation: `!out!/nix/store/....drv`.
    Built { drv: Rc<str>, output: Rc<str> },
}

impl ContextElem {
    /// How cppnix renders one element in an error, from
    /// `NixStringContextElem::display`: a bare store path, `=drv` for every
    /// output of a derivation, `!output!drv` for one of them.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            ContextElem::Opaque(p) => p.to_string(),
            ContextElem::DrvDeep(p) => format!("={p}"),
            ContextElem::Built { drv, output } => format!("!{output}!{drv}"),
        }
    }

    /// The same shapes, with each store path reduced to its name part.
    ///
    /// **Not a tidier [`ContextElem::display`], a different cppnix
    /// rendering.** `NixStringContextElem::to_string` (`value/context.cc:65`)
    /// writes `o.path.to_string()`, and a `StorePath`'s own `to_string` is
    /// `<hash>-<name>` with no store directory, so any message built from it
    /// names the base name. `forceStringNoCtx`'s failure is the other one and
    /// prints the whole path, which is why both exist rather than one being
    /// folded into the other. Quoted from nix 2.34.7+ix.g69e4d9e9db39:
    ///
    /// ```text
    /// toFile:            ... references !out!nrb4avj6...-d.drv
    /// forceStringNoCtx:  ... (such as '/nix/store/g76zcpqc...-a')
    /// ```
    #[must_use]
    pub fn display_base_name(&self) -> String {
        let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_owned();
        match self {
            ContextElem::Opaque(p) => base(p),
            ContextElem::DrvDeep(p) => format!("={}", base(p)),
            ContextElem::Built { drv, output } => format!("!{output}!{}", base(drv)),
        }
    }

    /// The inverse of [`ContextElem::display`], which is cppnix's
    /// `NixStringContextElem::parse` (`value/context.cc:9`).
    ///
    /// It exists because [`ContextElem::display`] is a wire format and not
    /// only an error rendering: a [`crate::task::NeedPath::Realise`] question
    /// travels to the embedder as these strings, and a witness naming one has
    /// to decode back into the same element or the question replayed is a
    /// different question. Recording and replaying through one pair of
    /// functions is what makes that impossible rather than merely unlikely.
    ///
    /// `None` for anything the renderer could not have produced. cppnix
    /// throws `BadNixStringContextElem` in the same three places -- an empty
    /// string, a leading `!` with no second `!`, and a `!` in a string that
    /// does not start with one -- and this crate has no error to raise here,
    /// because every caller of this is a decoder for which a malformed input
    /// is a miss rather than a program's fault.
    #[must_use]
    pub fn parse(s: &str) -> Option<ContextElem> {
        match s.as_bytes().first()? {
            b'!' => {
                let rest = s.get(1..)?;
                let (output, drv) = rest.split_once('!')?;
                // cppnix recurses here and rejects a nested `Built`, which
                // needs `dynamic-derivations`; this crate has no
                // representation for one, so a third `!` is malformed.
                if drv.contains('!') {
                    return None;
                }
                Some(ContextElem::Built {
                    drv: drv.into(),
                    output: output.into(),
                })
            }
            b'=' => Some(ContextElem::DrvDeep(s.get(1..)?.into())),
            _ => {
                if s.contains('!') {
                    return None;
                }
                Some(ContextElem::Opaque(s.into()))
            }
        }
    }
}

/// The context a value contributes to a string built out of it: a string's
/// own, and nothing for anything else. A path contributes nothing because a
/// coercion that does not copy creates no dependency, and one that does copy
/// answers with a string that already carries the element.
#[must_use]
pub fn context_of(v: &Value) -> BTreeSet<ContextElem> {
    match v {
        Value::Str(s) => s.context_set(),
        _ => BTreeSet::new(),
    }
}

/// A Nix string: the bytes, and the store paths the value depends on.
///
/// **Bytes, not text.** cppnix's `nString` is an arbitrary byte sequence --
/// `builtins.readFile` of a binary, `substring` sliced mid-codepoint,
/// `getEnv` of a variable that was never UTF-8 -- and every string operation
/// there is byte-oriented. Holding a Rust `str` here meant every such value
/// was either refused or repaired to U+FFFD before the program saw it
/// (ENG-13147, ENG-13146), which is a divergence no call site could observe
/// locally. So the representation is `Rc<[u8]>` and the places that
/// genuinely need text -- an attribute name for the interner, a path for the
/// host, a URL -- say so through [`NixStr::as_str`] and refuse loudly when
/// the bytes are not text, rather than every string paying a validation it
/// does not want.
///
/// The context is `Option<Rc<...>>` and not a plain set because the
/// overwhelming majority of strings in any evaluation have none, and a string
/// is the most frequently allocated value there is; `None` costs one null
/// pointer and no allocation.
///
/// Equality ignores the context, as cppnix's does: `"a" == "a"` whatever
/// either side depends on. That is also why the context is not part of `Ord`.
#[derive(Clone)]
pub struct NixStr {
    bytes: Rc<[u8]>,
    context: Option<Rc<BTreeSet<ContextElem>>>,
}

/// Text when the bytes are text, the raw byte array only when they are not:
/// almost every string a debugger meets is text, and a page of ASCII codes
/// hides the one byte that matters.
impl std::fmt::Debug for NixStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("NixStr");
        match self.as_str() {
            Some(text) => d.field("text", &text),
            None => d.field("bytes", &self.bytes),
        };
        d.field("context", &self.context).finish()
    }
}

impl NixStr {
    /// The bytes, with no claim about the context. Named rather than reached
    /// through `Deref` at the sites that matter, so a builtin dropping a
    /// context is visible in the source rather than only at runtime.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes, sharing the allocation.
    #[must_use]
    pub fn bytes_rc(&self) -> Rc<[u8]> {
        Rc::clone(&self.bytes)
    }

    /// The bytes as text, for the boundaries that are genuinely text-only:
    /// an attribute name headed for the interner, a path or URL headed for
    /// the host, a hash-algorithm name. `None` when the bytes are not UTF-8,
    /// and the caller decides what that means there -- usually a refusal,
    /// never a silent repair.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// The bytes as text when they are text, a loud marker around the lossy
    /// rendering when they are not. For tests and examples: an assertion
    /// comparing against real text fails visibly on the marker, with no
    /// panic path in the library. Production code goes through
    /// `primops_pure::text_of`, which refuses by name.
    #[must_use]
    pub fn expect_text(&self) -> String {
        match self.as_str() {
            Some(text) => text.to_owned(),
            None => format!("<non-UTF-8 NixStr: {}>", self.lossy()),
        }
    }

    /// The bytes rendered for a human, U+FFFD for what is not text. **Error
    /// messages and logs only**: anything a program or a hash can observe
    /// must take [`NixStr::bytes`], because this rendering is exactly the
    /// repair the byte representation exists to avoid.
    #[must_use]
    pub fn lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes)
    }

    #[must_use]
    pub fn context(&self) -> Option<&Rc<BTreeSet<ContextElem>>> {
        self.context.as_ref()
    }

    pub fn has_context(&self) -> bool {
        self.context.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// An owned copy of the context, for a builtin building a new string out
    /// of this one. Empty when there is none, so a caller can extend it
    /// without caring which case it started from.
    #[must_use]
    pub fn context_set(&self) -> BTreeSet<ContextElem> {
        self.context
            .as_ref()
            .map(|c| (**c).clone())
            .unwrap_or_default()
    }

    /// The same bytes with no context, which is `unsafeDiscardStringContext`.
    #[must_use]
    pub fn without_context(&self) -> NixStr {
        NixStr {
            bytes: Rc::clone(&self.bytes),
            context: None,
        }
    }

    /// The same bytes carrying `context`. `None` and an empty set are the same
    /// string, and are normalised to `None` so two equal strings cannot hash
    /// or print differently for a reason nobody can see.
    #[must_use]
    pub fn with_context(bytes: impl Into<Rc<[u8]>>, context: BTreeSet<ContextElem>) -> NixStr {
        NixStr {
            bytes: bytes.into(),
            context: if context.is_empty() {
                None
            } else {
                Some(Rc::new(context))
            },
        }
    }

    /// The same bytes carrying `context` in place of whatever this string
    /// had, sharing the byte allocation. What the context-rewriting builtins
    /// (`addDrvOutputDependencies`, `unsafeDiscardOutputDependency`,
    /// `appendContext`) hand back: all three keep `s` and replace the set.
    #[must_use]
    pub fn replacing_context(&self, context: BTreeSet<ContextElem>) -> NixStr {
        NixStr::with_context(Rc::clone(&self.bytes), context)
    }

    /// The union of several strings' contexts, for concatenation. cppnix's
    /// `copyContext` per part, which is what makes `"${a}${b}"` depend on
    /// everything `a` and `b` did.
    #[must_use]
    pub fn union_context<'a>(parts: impl Iterator<Item = &'a NixStr>) -> BTreeSet<ContextElem> {
        let mut out = BTreeSet::new();
        for p in parts {
            if let Some(c) = &p.context {
                out.extend(c.iter().cloned());
            }
        }
        out
    }
}

/// Bytes only, as cppnix's `eqValues` does for `nString`.
impl PartialEq for NixStr {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for NixStr {}

impl From<String> for NixStr {
    fn from(text: String) -> Self {
        NixStr {
            bytes: text.into_bytes().into(),
            context: None,
        }
    }
}

impl From<&str> for NixStr {
    fn from(text: &str) -> Self {
        NixStr {
            bytes: text.as_bytes().into(),
            context: None,
        }
    }
}

impl From<Vec<u8>> for NixStr {
    fn from(bytes: Vec<u8>) -> Self {
        NixStr {
            bytes: bytes.into(),
            context: None,
        }
    }
}

impl From<&[u8]> for NixStr {
    fn from(bytes: &[u8]) -> Self {
        NixStr {
            bytes: bytes.into(),
            context: None,
        }
    }
}

impl From<Rc<[u8]>> for NixStr {
    fn from(bytes: Rc<[u8]>) -> Self {
        NixStr {
            bytes,
            context: None,
        }
    }
}

impl From<Rc<str>> for NixStr {
    fn from(text: Rc<str>) -> Self {
        NixStr {
            // Allocation-free: std converts the `Rc` in place.
            bytes: text.into(),
            context: None,
        }
    }
}

#[derive(Debug)]
pub struct ClosureData {
    pub module: Rc<Module>,
    pub unit: u32,
    pub env: Env,
}

#[derive(Debug)]
pub struct BuiltinData {
    pub idx: u16,
    pub args: Vec<Slot>,
}

/// A lazily-evaluated cell: thunk until forced, then value forever.
#[derive(Debug, Clone)]
pub struct Slot(pub Rc<RefCell<SlotState>>);

#[derive(Debug)]
pub enum SlotState {
    Value(Value),
    Thunk {
        module: Rc<Module>,
        unit: u32,
        env: Env,
    },
    /// Under evaluation: hitting this is infinite recursion.
    Blackhole,
    /// A forced thunk whose evaluation threw. Re-forcing rethrows the same
    /// error (cppnix memoizes failures the same way).
    Failed(Rc<crate::vm::Catchable>),
    /// `f a b` not yet performed. cppnix's `mkApp` stores an **unforced**
    /// left-hand side and forces it at the application, so the callee here is
    /// a `Slot` and not a `Value`.
    ///
    /// The distinction is observable, not an internal detail:
    /// `builtins.mapAttrs` never forces its function (`primops.cc`,
    /// `prim_mapAttrs` forces only the set), so `mapAttrs throw attrs` is a
    /// set of unexploded thunks. Holding a forced `Value` here made the
    /// builtin strict in its function, which is what turned nixpkgs'
    /// `idrisPackages` self-reference -- `{ ... } // mapAttrs
    /// self.build-builtin-package { ... }` -- into `infinite recursion
    /// encountered` where cppnix succeeds (ENG-13124).
    PendingApply {
        f: Slot,
        args: Vec<Slot>,
    },
    /// A builtin (or other feature) the evaluator does not implement yet;
    /// forcing reports it as unimplemented, which the harnesses count
    /// separately from mismatches. The whole [`crate::refusal::Refusal`]
    /// is kept, not just the prose: a refusal memoized here by a spawned
    /// strand (ENG-13150) must re-raise with its original token, because
    /// the census groups by token and the sequential evaluation would
    /// have raised the original.
    Unimplemented(Rc<crate::refusal::Refusal>),
}

impl Slot {
    pub fn value(v: Value) -> Self {
        Slot(Rc::new(RefCell::new(SlotState::Value(v))))
    }

    pub fn thunk(module: Rc<Module>, unit: u32, env: Env) -> Self {
        Slot(Rc::new(RefCell::new(SlotState::Thunk {
            module,
            unit,
            env,
        })))
    }

    pub fn pending(f: Slot, args: Vec<Slot>) -> Self {
        Slot(Rc::new(RefCell::new(SlotState::PendingApply { f, args })))
    }

    pub fn unimplemented(what: &str) -> Self {
        Slot(Rc::new(RefCell::new(SlotState::Unimplemented(Rc::new(
            crate::refusal::Refusal::new(crate::refusal::RefusalToken::UnimplementedBuiltin, what),
        )))))
    }

    /// The memoized value, or `None` when this slot has not been forced.
    /// The machine forces every value a builtin is allowed to look at, so a
    /// builtin reading `None` is an interpreter bug, never a Nix-level one.
    pub fn peek(&self) -> Option<Value> {
        match &*self.0.borrow() {
            SlotState::Value(v) => Some(v.clone()),
            _ => None,
        }
    }

    /// Identity of the cell, for cycle detection in deep traversals.
    pub fn id(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }
}

/// One Nix expression routinely builds a value nested tens of thousands of
/// levels deep, and the derived drop glue recurses once per level, so a
/// teardown would blow the host stack right after an evaluation that never
/// touched it. Dismantle iteratively instead: take each cell's state out
/// behind a leaf and push its children onto a worklist.
impl Drop for Slot {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) != 1 {
            return;
        }
        let Ok(mut here) = self.0.try_borrow_mut() else {
            return;
        };
        let mut work = vec![Junk::State(std::mem::replace(
            &mut *here,
            SlotState::Blackhole,
        ))];
        drop(here);
        while let Some(j) = work.pop() {
            match j {
                Junk::State(SlotState::Value(v)) => dismantle_value(v, &mut work),
                Junk::State(SlotState::Thunk { env, .. }) => work.push(Junk::Env(env)),
                Junk::State(SlotState::PendingApply { f, args }) => {
                    for s in &args {
                        drain_slot(s, &mut work);
                    }
                    drain_slot(&f, &mut work);
                }
                Junk::State(_) => {}
                Junk::Env(e) => dismantle_env(e, &mut work),
            }
        }
    }
}

enum Junk {
    State(SlotState),
    Env(Env),
}

/// Empty a slot we are the last owner of, queueing whatever it held. The
/// slot itself is left holding a leaf, so its own `Drop` is then trivial.
fn drain_slot(s: &Slot, work: &mut Vec<Junk>) {
    if Rc::strong_count(&s.0) != 1 {
        return;
    }
    if let Ok(mut st) = s.0.try_borrow_mut() {
        work.push(Junk::State(std::mem::replace(
            &mut *st,
            SlotState::Blackhole,
        )));
    }
}

fn dismantle_value(v: Value, work: &mut Vec<Junk>) {
    match v {
        Value::List(rc) => {
            if let Ok(items) = Rc::try_unwrap(rc) {
                for s in &items {
                    drain_slot(s, work);
                }
            }
        }
        Value::Attrs(rc) => {
            if let Ok(map) = Rc::try_unwrap(rc) {
                for s in map.values() {
                    drain_slot(s, work);
                }
            }
        }
        Value::Closure(rc) => {
            if let Ok(c) = Rc::try_unwrap(rc) {
                work.push(Junk::Env(c.env));
            }
        }
        Value::Builtin(rc) => {
            if let Ok(b) = Rc::try_unwrap(rc) {
                for s in &b.args {
                    drain_slot(s, work);
                }
            }
        }
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Null
        | Value::Str(_)
        | Value::Path(_) => {}
    }
}

fn dismantle_env(e: Env, work: &mut Vec<Junk>) {
    let mut cur = e;
    loop {
        match Rc::try_unwrap(cur) {
            Ok(EnvNode::Frame { up, slots }) => {
                for s in slots.borrow().iter() {
                    drain_slot(s, work);
                }
                cur = up;
            }
            Ok(EnvNode::With { up, subject }) => {
                drain_slot(&subject, work);
                cur = up;
            }
            Ok(EnvNode::Root) | Err(_) => return,
        }
    }
}

/// Environment: a chain of frames, innermost last. Frames are shared, not
/// copied, when captured by thunks and closures.
pub type Env = Rc<EnvNode>;

#[derive(Debug)]
pub enum EnvNode {
    Root,
    Frame {
        up: Env,
        slots: RefCell<Vec<Slot>>,
    },
    /// A `with` scope. Kept in the same chain so PopEnv/PopWith stay
    /// balanced under laziness (thunks capture whatever chain existed).
    With {
        up: Env,
        /// The with subject, lazily forced on first dynamic resolve.
        subject: Slot,
    },
}

pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "an integer",
        Value::Float(_) => "a float",
        Value::Bool(_) => "a Boolean",
        Value::Null => "null",
        Value::Str(_) => "a string",
        Value::Path(_) => "a path",
        Value::List(_) => "a list",
        Value::Attrs(_) => "a set",
        Value::Closure(_) | Value::Builtin(_) => "a function",
    }
}

/// Lexical path normalization ("." and ".." segments); never touches the
/// filesystem, same as cppnix's path handling. Applied both to path literals
/// at compile time and to the result of `path + string`, which is why
/// `/foo/bar + "/../xyzzy/." + "/foo.txt"` is `/foo/xyzzy/foo.txt`.
pub fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    format!("/{}", out.join("/"))
}

/// printf `%.6g`: how cppnix **prints** a float, in `printValue` and
/// everything built on it.
///
/// Not how it **coerces** one. cppnix has two float renderings and they
/// disagree on almost every value -- `coerceToString` uses
/// `std::to_string(double)`, which is `%f` (see [`format_f6`]). Store paths
/// depend on the coercion and never on this, so a comment here once claiming
/// that "drv hashes depend on it" was pointing at the wrong function; the
/// hashes depend on `format_f6`.
pub fn format_g6(x: f64) -> String {
    if x == 0.0 {
        return "0".to_owned();
    }
    let exp = x.abs().log10().floor() as i32;
    if (-5..6).contains(&exp) {
        let prec = usize::try_from(5i64 - i64::from(exp)).unwrap_or(0);
        let mut out = format!("{x:.prec$}");
        if out.contains('.') {
            while out.ends_with('0') {
                out.pop();
            }
            if out.ends_with('.') {
                out.pop();
            }
        }
        out
    } else {
        let s = format!("{x:.5e}");
        let Some((mantissa, e)) = s.split_once('e') else {
            return s;
        };
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        format!("{mantissa}e{e:+03}")
    }
}

/// `std::to_string(double)`, which is how cppnix **coerces** a float to a
/// string (`eval.cc:2657`): `%f`, so exactly six digits after the point, never
/// an exponent, and no round trip -- `1.0` coerces to `"1.000000"` and `1e10`
/// to `"10000000000.000000"`.
///
/// This one is hashed. A float attribute of a derivation goes through
/// `coerceToString` into an environment variable and from there into the
/// `.drv` and every store path below it, so rendering it as [`format_g6`]
/// does (`"1"`, `"1e+10"`) produced a derivation that was well-formed, stable,
/// and not cppnix's -- measured as a wrong `outPath` against the cpp backend
/// on dev-compute-4.
pub fn format_f6(x: f64) -> String {
    format!("{x:.6}")
}

impl fmt::Display for Value {
    /// Debug-ish display for errors; the real corpus printer lives in
    /// `print` (it needs the interner for attr names and forces lazily).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(x) => write!(f, "{}", format_g6(*x)),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Null => write!(f, "null"),
            // Debug rendering for errors, so lossy is honest here: the raw
            // bytes go through `print`, never through `Display`.
            Value::Str(s) => write!(f, "\"{}\"", s.lossy()),
            Value::Path(p) => write!(f, "{p}"),
            Value::List(_) => write!(f, "[ ... ]"),
            Value::Attrs(_) => write!(f, "{{ ... }}"),
            Value::Closure(_) | Value::Builtin(_) => write!(f, "<LAMBDA>"),
        }
    }
}
