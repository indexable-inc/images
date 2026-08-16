# Source positions in the Rust evaluator

**What you get.** An error from `eval-backend=rust` names the line and column
it happened on, in cppnix's format, and `builtins.unsafeGetAttrPos` answers a
real record instead of `null`. ENG-12137. Before this, the crate carried no
positions at all: every error read `error: <message>` with no `at` line, and
that builtin answered `null` for everything under an owner-approved
divergence (ENG-12591), now retired along with its three allowlist entries.

**What you do not get.** Traces. cppnix prints a chain of `… while calling
the 'head' builtin` frames, each with its own position; this crate carries one
position, the innermost. ENG-12714 tracks the chain. Two attribute cases below
answer `null` where cppnix answers a record; both are named and pinned.

## How it is stored: a side table, not a wider instruction

`ir::Op` is a fixed-width `Copy` enum the interpreter fetches in its innermost
loop. Widening it by four bytes to serve a path that only runs when an
evaluation is already failing would charge every evaluation that does not, and
this crate had just recovered 37.5% of cold-eval CPU.

So each `CodeUnit` carries `spans: Vec<u32>`, parallel to `ops`, holding the
byte offset of the construct that emitted each op (or `ir::NO_POS`, which is
`u32::MAX` and not `0`, because `0` is the first byte of the file). The
`Module` carries `line_starts: Vec<u32>` so an offset resolves to a line and a
column with no IO -- the VM performs none, so it cannot re-read the file the
way cppnix's `PosTable::operator[]` does. The source text itself is not kept:
the only thing the VM would ever do with it is count newlines, which is that
array.

Nothing in the interpreter reads `spans` on a successful run. The one cost on
the hot path is `self.at_ip = u.ip` once per op in `Vm::advance_unit`, a store
to a field already in cache.

`unsafeGetAttrPos` needs a second table, because it asks about an attribute
reached through a *value* rather than about an instruction. `CodeUnit::attr_sites`
records, per `MkAttrs`, the statically known names that op builds and where
each was written, sorted for binary search; `value2::Attrs` carries an
`AttrOrigin { module, unit, ip }` naming the op that built it. That is 16 bytes
and one refcount bump per attribute set on the heap, and nothing pays it per
value: widening `Value` itself would have.

## What has a position

Errors, everywhere the VM raises them. Attribution happens in `Vm::advance`,
for every frame kind and not only `Frame::Unit` -- `throw`, `abort` and a
builtin's argument type errors all raise from inside a `Frame::Task`, and
those are the errors users see most.

Every op the compiler emits except the synthesised `Ret`, which has no token
and cannot fail. Measured over the corpus in `compile::span_tests`: 331 ops,
250 positions, all 81 gaps a `Ret`.

Attributes, for a set that came from source: a literal, a `rec` literal, an
`inherit` list, a dynamic `${e} =` binding, and every component of a nested
attrpath. Formal parameters, so `builtins.functionArgs` answers the way cppnix
does. Derived sets take the origin of the operand whose values they take --
`//` the right, `removeAttrs` its own, `intersectAttrs` the second -- so a
reported position is never wrong, only sometimes absent.

The `nix-eval-driver` entry point (#184) does not print positions. Its
failure text is compared byte for byte against the C++ CLI's by
`rust-driver-parity.sh`, and cppnix's CLI puts no `at file:line:col` inside
that string, so `run.rs::failure_of` drops the position on purpose rather than
for want of one. The bridge path (`eval-backend = rust` under `nix` and
`nix-instantiate`) is the one that renders them.

## What answers `null`

Two of these match cppnix and three do not. All three divergences are `null`
where cppnix has a record, so each is a missing answer rather than a wrong one,
and each is pinned by a test in `rust/nix-eval-rs/tests/positions.rs`.

| case | cppnix | here | |
|---|---|---|---|
| attribute not in the set | `null` | `null` | matches |
| text with no file behind it (`--expr`, the REPL) | `null` | `null` | matches (`eval.cc`'s `mkPos` builds a record only for a `SourcePath` origin) |
| `{ a = 1; } // { b = 2; }`, asking for `a` | column of the left `a` | `null` | **divergence** |
| `rec { __overrides = { a = 20; }; a = 1; b = 2; }`, asking for `b` | column of the rec's `b` | `null` | **divergence**, the same one |
| `builtins.listToAttrs [ { name = "a"; value = 1; } ]` | column of `value` | `null` | **divergence** |

The `__overrides` row is worth naming because nothing in that source says
`//`: `compile::emit_rec_set_build` closes the statics into one set and
appends the override set with an `Update`, so the rule above applies and the
result takes the override set's origin. The overridden attribute itself
therefore answers cppnix's column exactly -- cppnix reads it from the override
set too -- and only a static the override does not name comes back `null`.

All three are the same limitation: **one origin per set, not one per
attribute.** cppnix stores a position on every `Attr`, so a set assembled from
several sources carries several unrelated positions. Storing that here means
either a position in every `Slot`, which is a per-attribute cost on every
evaluation, or an origin *chain* per set, which a fold of `//` grows without
bound. Neither was worth spending against the perf budget for a builtin whose
answer is advisory. If it becomes worth it, the chain is the cheaper of the
two and `AttrOrigin::offset_of` is where it goes.

## Reading a position back out

`SrcPos { file: Option<Rc<str>>, line, column }` travels out of the crate three
ways, all added by ENG-12137: `EvalError::pos()`, the `IxePos` out-parameter on
`ixe_eval_expr` and `ixe_session_take_error`, and a `"pos"` key in the cached
`EvalResult` (an empty array means none, so an old row decodes without a
position rather than failing).

On the C++ side `rustEvalPos` turns that back into a `std::shared_ptr<const
Pos>` and `rustEvalThrow` attaches it, which is what makes the `at
/path:LINE:COL:` line and its source excerpt appear. That needed a new public
`EvalErrorBuilder<T>::atPos(std::shared_ptr<const Pos>)`: `ErrorInfo err` is
protected, and the existing `atPos` overloads all take a `PosIdx` into
cppnix's own `PosTable`, which an embedder computing positions elsewhere has
no way to produce.

## Columns are bytes, and lines end three ways

cppnix's column is `1 + (offset - lineStart)` over a **byte** offset, so a line
with multi-byte characters before the column reports the same number on both
evaluators only if this one also counts bytes. It does; `columns_count_bytes`
pins it with an `é`.

A line ends at `\n`, at `\r\n`, or at a bare `\r`, which is what
`Pos::LinesIterator` accepts. Counting only `\n` would drift on any file
written with `\r`, silently and only on those files.

## Verifying against cppnix on macOS: use `/private/tmp`

`/tmp` is a symlink to `/private/tmp` and cppnix does not resolve it, so
`SourcePath::readFile()` throws, `getSource()` returns nullopt, and
`PosTable::operator[]` degenerates to `lines = [0]`: **every position reports
line 1 and column `offset + 1`, with no source excerpt.** It looks like a
positions bug and it is not one -- the system nix does the same. Any oracle run
under `/tmp` is measuring that instead of what you asked. Every expectation in
`tests/positions.rs` was taken under `/private/tmp`.
