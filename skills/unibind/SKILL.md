---
name: unibind
description: "How ix generates its SDKs: unibind lowers one Rust surface into TypeScript, Python, JVM and Elixir bindings, and the rule is that the IR carries intent while each backend renders its own language's idiom. Use when changing the SDK surface (crates/ix/sdk-bind), adding or fixing a unibind feature, or when a generated API reads unidiomatically in any language."
---

## The rule

One Rust surface, four idiomatic SDKs. Not four transliterations of one.

The IR carries **intent**: this constructs the object, this owns something that
must be released, this is a stream, this is a closed set of variants. Each
backend renders its own language's idiom for that intent. A shape that only one
language has never reaches the IR.

The failure mode this exists to prevent is lowest-common-denominator design.
When the IR could only say "free function", "method with a receiver" and "sync
constructor", every language got

    client.machines().create({ template: "docker.io/library/python:3.12" })

because that was the only sentence all four backends could speak. The idiomatic
call is `Machine.oci(...)` in all four, and it looks different in each: a static
method returning a promise, a static coroutine, a static method returning a
future, a module function returning a tagged tuple. One intent, four renderings.

Reviewing a generated surface, ask what a native library in that language would
look like, not whether the shape matches the Rust. A `.d.ts` that reads like
Rust with braces is a defect, and so is a `.pyi` that reads like TypeScript.

## Intent table

| Intent | Rust | TypeScript | Python | Kotlin | Elixir |
|---|---|---|---|---|---|
| construct, async | `#[unibind(associated)]` `async fn` returning `Self` | `static oci(): Promise<Machine>` | `@staticmethod async def oci() -> Machine` | `suspend fun` in a `companion object` | `Machine.oci(...) -> {:ok, machine}` |
| release at scope end | `object(resource)` + `close` | `Symbol.asyncDispose`, so `await using` | `__aenter__`/`__aexit__`, so `async with` | `AutoCloseable`, so `use { }` | nothing; the BEAM drops the `ResourceArc`, or take a function |
| closed variant set | `#[unibind::enumeration]` `enum Status { Running, .. }` | `export type Status = "running" \| "stopped"` | `enum.StrEnum` | `enum class` | atoms |
| variants with data | `enum Frame { Phase{..}, Done{..} }` | discriminated union, exhaustive `switch` | frozen dataclasses under a `Literal` tag, `match` | sealed interface over data classes, exhaustive `when` | tagged tuples, `case` |
| identity | `MachineId(Uuid)` | branded string | `NewType` | `@JvmInline value class` | opaque type |
| optionality | `Option<T>` | `T \| null` | `T \| None` | `T?`, checked by the compiler | `nil` |
| sequence | `UniStream<T>` | `AsyncIterable` | `__aiter__`/`__anext__` | `Flow<T>` | `Stream` |
| failure | error enum | error subclasses | exception hierarchy | sealed exception hierarchy | `{:error, reason}` |
| a verb on the type | `#[unibind(associated)]` returning anything else | `static list(): Promise<Machine[]>` | `@staticmethod async def list()` | `companion object` | module function |

One flag covers both associated rows, and the return type decides the
rendering rather than the author: napi needs `factory` to build an instance
from an async static and plain `#[napi]` for everything else, so
`Machine.oci` and `Machine.list` are written the same way and come out
different. Saying it twice is a way to say it inconsistently.

### One wire spelling, four type names

The closed-variant row is the one intent where the languages must agree on a
*value*, not only on a shape: the same string is what TypeScript compares, what
a Python `StrEnum` member holds, and what the service already puts in its JSON.
So the rule splits in two.

**The wire spelling is decided once, at lowering, and is the same in every
language.** It defaults to `snake_case` of the Rust variant, and
`#[unibind::enumeration(rename_all = "...")]` picks another convention for the
whole enum. The conventions and their output are serde's, byte for byte, so a
binding cannot disagree with the JSON `#[serde(rename_all = ...)]` produces on
the same enum. `MachineProgress.kind` is `PascalCase` on the wire where every
other closed set in the ix surface is `snake_case`, and that is exactly one
word (`rename_all = "PascalCase"`) rather than a guess in each backend.

**The type name, and any member identifier, is idiomatic per language.** The
type keeps the Rust `PascalCase` name unless `ts(name = ...)` / `py(name = ...)`
renames it, like every other declared type. A member identifier only exists
where the language has one: Python spells members `SCREAMING_SNAKE_CASE`
(`MachineStatus.HARD_FAILURE == "hard_failure"`), and a TypeScript union has no
identifier at all, which is why per-variant renames are Python's business and
the ts backend ignores them.

TypeScript gets a union of string literals, never a TS `enum`: what crosses is
a plain string, a union erases at compile time where an `enum` needs a runtime
object, and `JSON.parse` output is assignable to a union and not to an `enum`.
Python gets `enum.StrEnum` (3.11+) because its member *is* a `str`, so
`isinstance`, `is`, and a caller's pre-existing `status == "running"` are all
true of one value; the extension builds the class at `#[pymodule]` init from
the same IR the `.pyi` declares, so there is no second source for the members.

Variants that carry data are a different intent (the row below) and lowering
still refuses them, naming the enum and the variant.

Kotlin's async row is not just `suspend fun`. Structured concurrency is the
contract: a cancelled coroutine must abort the Rust future rather than
leaving it running, so a suspending binding is written with
`suspendCancellableCoroutine` plus `invokeOnCancellation`, which is the
Kotlin end of the same abort path the TypeScript backend drives from an
`AbortSignal`. Streams are `Flow`, which is cold, cancellable and
backpressured by construction, so it carries the same contract without a
second mechanism. Scope-bound release is `use { }` on `AutoCloseable`, the
`with` of that language. Get those three right and a generated Kotlin SDK
composes with `coroutineScope` the way a hand-written one would; get them
wrong and every caller leaks a Rust task on the first cancellation.

The JVM column says Kotlin, not Java, and that is a decision rather than a
preference. Java has no answer for the async row at all, which is why the
backend's own rejection message tells the caller to block on a runtime and
move the call to a virtual thread. Kotlin answers every row natively. The
evidence that settled it: the only consumer of the JVM backend in either
repo is `index/packages/minecraft/probe-kt`, which is Kotlin, and whose
header describes what it consumes as "the unibind-rendered **Java** class".
We generated Java for a Kotlin caller. `index/lib/languages/kotlin.nix`
already builds that probe with `-Werror`, so the toolchain is in place, and
the backend is the least-built of the four, so re-basing it costs less now
than it ever will again.

Elixir keeps the table honest. It has no classes to hang a static method on and
no scope-bound cleanup at all, so any IR concept that survives contact with
Elixir is intent rather than syntax. When a new concept has no Elixir answer,
that is a signal the concept is a Node or Python fact wearing a general name.

## Types are the product

A generated type that says `string` where the value is one of ten known words
has moved the specification into a doc comment, where no compiler reads it. Two
rules follow.

**A closed set is an enum, never a string.** `status`, `kind`, `phase`, `level`
and every other field whose doc comment lists its legal values belongs in a Rust
enum, so each backend can render its own closed type.

**Variants that carry different data are one sum type, never one struct with
optional fields.** The anti-pattern in our own surface today is
`SwitchProgress`: a `kind: String` beside `phase`, `stdout`, `system` and
`error`, each set on a different frame, every one optional because none of them
is always there. The caller reconstructs by hand (`if (frame.finished &&
frame.system)`) what a tagged union would have decided at compile time. It is a
Rust enum flattened into a struct because the IR had nowhere to put it.

## State of the backends

Do not promise a surface a backend cannot render. As of 2026-08-04:

| | TypeScript | Python | JVM | Elixir |
|---|---|---|---|---|
| objects | yes | yes | **rejected outright** (`backend-jvm/src/module.rs`), no handle registry | yes, as `ResourceArc` handles |
| async | yes | yes | **rejected outright** (`backend-jvm/src/function.rs`) | free functions only; object members rejected (`backend-ex/src/object.rs`) |
| associated functions | yes; `#[napi(factory)]` when it returns the object, plain static otherwise | yes, `#[staticmethod]` | no | no |
| resource close | `close()` + leak warning | `close` + `__aenter__`/`__aexit__` + `ResourceWarning` | none | flag ignored; the BEAM drop runs `Drop` |
| scope-bound release | `[Symbol.asyncDispose]` in the generated JS wrapper, so `await using` works | `async with` | none | n/a |
| unit enums | yes; union of string literals, plus `z.enum` | yes; `enum.StrEnum` | **rejected** (`backend-jvm/src/module.rs`); owes a Kotlin `enum class` | **rejected** (`backend-ex/src/module.rs`); owes an atom per variant |
| data enums | **rejected by lowering**, naming the variant | same | same | same |
| runtime validation | Zod schemas, `z.infer` types | **none**; Pydantic models are the gap | no | no |
| error enums | yes | yes | yes | yes |

Two notes on that table. napi-rs itself cannot codegen `Symbol.asyncDispose`
and has no attribute for it, but unibind emits its own JS wrapper class over
the napi addon, and that is where dispose is written, so `await using` works
today. And the Zod row is the model for the Python gap: TypeScript consumers
get runtime validation generated from the same IR that types them, Python
consumers get none.

So the shipped ix SDKs are TypeScript and Python. The JVM and Elixir backends
serve other unibind consumers, and their gaps are stated in their own rejection
messages, which are worth reading before designing around them.

## Examples are copied, so they are house style

An SDK example is read as a template, not as a demo, so anything hand-rolled
in one becomes a pattern in somebody's codebase. Argument parsing is where
this shows up first: reach for the language's real parser, never index into
the raw argv.

| | parser |
|---|---|
| TypeScript | `parseArgs` from `node:util`, stable since Node 20, so no dependency |
| Python | `argparse`, as `packages/minecraft/probe/mc_probe.py` already does |
| Rust | `clap` |
| Kotlin | `kotlinx-cli`, or `clikt` where subcommands earn it |

`process.argv[2]` and `sys.argv[1]` accept anything, report nothing, and
teach the reader to do the same. The same rule covers the rest of an
example's surface: use the standard library's path, time and JSON handling
rather than string manipulation, because the example is the documentation
that gets executed.

## A doc link resolves, or the build fails

Doc comments reach four published surfaces, so a rustdoc link in one is a
claim about the *generated* API, not about Rust. unibind resolves every
intra-doc link against the interface and renders it in each language's own
spelling:

| written | TypeScript | Python |
|---|---|---|
| ``[`Machine`]`` | `{@link Machine}` | `` `Machine` `` |
| ``[`Machine::forward_port`]`` | `{@link Machine.forwardPort}` | `` `Machine.forward_port` `` |
| ``[`MachineInfo::failure_reason`]`` | `{@link MachineInfo.failureReason}` | `` `MachineInfo.failure_reason` `` |
| ``[`SnapshotStatus::Ready`]`` | `` `"ready"` `` | `` `SnapshotStatus.READY` `` |
| ``[`IxError::NotFound`]`` | `{@link NotFound}` | `` `NotFound` `` |
| ``[`Self::close`]`` | resolved against the enclosing type | same |

TypeScript gets TSDoc, which editors resolve; Python gets a code span,
because a `.pyi` has no link syntax a reader's tooling follows and Sphinx
roles would be a second vocabulary in a file nobody runs Sphinx over. An
enumeration variant is the one target with no identifier in TypeScript -- a
union member is a string -- so it renders the value a caller actually types.
Elixir and the JVM render code spans too, and have no conformance coverage
for links yet.

**A link that names nothing fails generation**, naming the doc site and the
dead target, the way rustdoc's `broken_intra_doc_links` does at `deny`. Every
dead link is reported in one build, because a rename usually leaves several.
That gate is the point: a denylist of one renamed noun (which is what
`artifact-sdk-vocabulary` was) cannot catch a link to a method that never
existed, and 214 dead links shipped in each published file before it existed.

Three spellings and what they do:

- ``[`Type::member`]`` is the form to write.
- `[the docs](Type::member)` resolves the same way, and **the link text is
  dropped** -- the target's spelling replaces the whole link, so write the
  sentence around the reference rather than through it.
- ``[`Type::member`][label]``, the reference form, is **refused**: its target
  lives in a link definition, and nothing writes those into a `.d.ts` or a
  `.pyi`, so it would ship as dead text with no error.

Two things that are deliberately not links: a Rust-side item the SDK reader
cannot see (write `` `ix_sdk::SdkError` `` as a plain code span; brackets
would claim it is part of the surface), and a bare `[1]` in prose. A
constructor link renders as its type (``[`Session::new`]`` becomes `{@link
Session}`), because no target language names a constructor -- so phrase the
sentence for that rather than writing "as [`Self::new`] received it".

Records are the one doc site where the runtime text and the stub could
disagree, because a record's `#[pyclass]` lands on the user's own struct and
pyo3 reads its `///` text. The macro writes the resolved lines back over
those attributes, so `help(Point)` and the `.pyi` say the same thing.

## One surface, several files

`#[unibind::export]` lowers one module, and lowering has to see all of it at
once: a type reference in a signature is classified against every declaration
in the export, so no per-file pass could resolve `MachineInfo` without the
file that declares it. The export therefore names its other files:

```rust
#[unibind::export(parts = ["src/sdk/machines.rs", "src/sdk/snapshots.rs"])]
mod _ix_sdk {}
```

Each listed file is a list of items -- the same items that would have been
written inline, at the file's own indentation -- read and appended to the
module before lowering. **Declaration order is the module's own items
followed by the parts in listed order**, which matters because the generated
layout mirrors it: the list is the crate author's statement of that order,
not filesystem order and not the order macros expand in.

Three compile errors keep the list honest:

- a part listed twice, naming it;
- a listed path that does not exist, naming it;
- **a `.rs` file that sits with the parts but is not listed**, naming it.
  Adding a file and forgetting to register it is the failure this catches; it
  would otherwise be silently absent from the SDK.

So keep parts in a directory of their own, and put nothing else there.

The cost, stated plainly: rustc does not read the part files, the macro does,
and stable Rust gives a proc macro no way to manufacture a span into a file
it read. A type error inside a part is reported against the
`#[unibind::export]` attribute; the offending code is still printed, but the
file and line are not the part's. unibind's own diagnostics name the item and
the part path instead.

**Add to the file that owns the namespace.** The old rule in
`crates/ix/sdk-bind` was to append a new `impl` block at the end of the one
file rather than edit the blocks above, because a module boundary was not
available and several agents write here at once; that produced six `impl
Machine` blocks spread over 5,000 lines. The file boundary does that job now:
a machines verb goes in `machines.rs`, and `Machine`'s verbs all live in
`machine.rs`. Within a file, appending to the existing `impl` block is fine.

Reordering items across parts is not free. Records, enumerations, errors and
objects each keep their declaration order in the IR, and each object's
methods keep the order of the `impl` blocks that contribute them, so moving
one of those changes two published files while changing nothing. Private
items carry no IR position and move freely.

## Adding an intent

The IR's function kind lives in lowering, not in the IR data: `Kind` in
`core/src/lower/func.rs` is `Free | Method | Constructor`, and an object carries
`constructor: Option<Function>` beside `methods: Vec<Function>`, so kind is
positional. A new intent is therefore a new arm there plus a new field on
`ir::Object`, and then one rendering decision per backend:

1. `core/src/lower/func.rs`, `core/src/lower/object.rs`: accept the shape, and
   say why in the rejection message for the shapes still refused.
2. `core/src/ir/data.rs`: carry it.
3. Each backend: render the local idiom, or refuse with a message naming the
   idiom it would have rendered. A backend that cannot do it yet refuses
   loudly; it never falls back to a shape from another language.
4. Each conformance suite (`conformance/`, `conformance-ts/`, `conformance-jvm/`,
   `conformance-ex/`): the fixture Rust surface plus assertions in the target
   language. A feature with no conformance test in a language is not supported
   in that language, whatever the renderer does. A lowering feature that is
   not language-specific still needs one suite exercising it: `conformance-ex`
   is split over `src/surface/*.rs`, which is what keeps `parts` covered.
5. Break it and watch it fail. A renderer's snapshot test passes just as well
   against the wrong output.

Anything that has to be hand-written on top of a generated surface is a bug
report against unibind, not a layer to grow. `packages/sdk/src/repl.ts` is the
one sanctioned exception, and it earns it by being protocol logic rather than a
second spelling of a generated verb.
