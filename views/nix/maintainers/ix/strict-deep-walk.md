# Should `toXML`, `toJSON` and `deepSeq` share one strict deep walk?

Yes for the two renderers, no for `deepSeq`. Two machines, not one and not
three. This is the comparison that decides it, written before any code, because
the alternative was starting a third hand-rolled walk on instinct.

## Why the question exists

`builtins.toXML` is unimplemented and is the next wall on a grub-booted NixOS
toplevel (ENG-12863). It is a strict deep walk of a value that renders text as
it goes. So are `builtins.toJSON` and, minus the text, `builtins.deepSeq`.
Three hand-rolled deep walks in one evaluator is the drift shape this repo's
CLAUDE.md names: one implementation per rule, or they diverge and only one gets
each fix.

## What the three actually are, read out of the code

| | `ToJson` (`primops_host.rs:188`) | `Cont::DeepSeq` (`primops_pure.rs:156`) | `printValueAsXML` (`value-to-xml.cc`) |
|---|---|---|---|
| output | `out: String` | none | nested elements |
| interleaving | `Job::Lit` for separators and closers | none needed | needs it, for closing tags |
| depth | per-`Job`, `MAX_DEPTH` 10,000 | **none** | `addCallDepth` per level |
| guard | none | `seen` on cell identity | `drvsSeen` on the drvPath **string** |
| suspends for | `Force`, `Apply` (`__toString`), `Need` (store copy) | `Force` only | `Force` (incl. two named attrs) |
| context | accumulated | n/a | accumulated |
| static IR needed | no | no | yes, lambda formals |
| size | ~170 lines | ~20 lines | 209 lines of C++ |

Three different guards is the first surprise, and none of them is a mistake:
cppnix's `printValueAsJSON` has no `seen` either, `forceValueDeep` keys on
`Value*` (`eval.cc:2418`), and `drvsSeen` is not a cycle guard at all -- it
dedups derivations and still emits an element, `<repeated/>`, in place of the
children.

## The derivation arm is not the problem it looks like

`<derivation drvPath="..." outPath="...">` needs both attribute values in hand
*before* the opening tag is written, so the renderer has to force two named
attributes and be called again with their values. That reads like a special
case that would contort a shared driver.

It is not, because `ToJson` already needs exactly that shape three times.
`Await::ToStrFn`, `Await::ToStrResult` and `Await::ToStrCoerced` are all
"suspend, get a value that is not the node's rendering, resume the renderer".
The derivation arm is a fourth instance of a pattern the existing machine was
already built around. Generalizing those three into a renderer-defined
auxiliary step is the shared design, not a concession to XML.

## `deepSeq` is the one that does not fit

It has no output. As a client of a driver that owns `out: String` and a `Lit`
job kind, it would carry both and write neither, and its twenty lines would
become a trait impl over a much larger thing to save nothing. Its guard is also
the only identity-based one, so the driver would need a guard policy that two
of three clients do not use.

The one thing it would gain is a depth limit, and it is genuinely missing one:

```
$ nix-instantiate --eval --strict -E \
    'let f = n: if n == 0 then [] else [ (f (n - 1)) ]; in builtins.deepSeq (f 20000) 1'
  cpp   rc=1  error: stack overflow; max-call-depth exceeded
  rust  rc=0  1
```

cppnix's `forceValueDeep` opens with `addCallDepth` (`eval.cc:2421`); the
crate's `Cont::DeepSeq` has no counter. That is a semantic divergence, filed
separately, and it is a four-line fix to `Cont::DeepSeq` rather than a reason
to rebuild it as something else. Fixing a bug by making the buggy thing a
client of a new abstraction is how a refactor smuggles in behaviour changes
nobody reviewed.

## What is given up by not folding `deepSeq` in

The depth-limit rule then has two implementations: the driver's and
`Cont::DeepSeq`'s. That is real and worth naming. It is bounded -- one integer
compared against one constant -- and the alternative costs more, so a test
asserts the two agree on the limit rather than a shared code path enforcing it.

## How much of the two renderers is actually shared

Going type by type, because "they are both deep walks" is not a measurement.
Six of nine arms differ in *behaviour*, not only in the text they emit:

| value | `toJSON` | `toXML` | same? |
|---|---|---|---|
| int, bool, null, float | scalar text | `<int value="N" />` etc. | text only |
| string | escaped, context copied | `<string value="..." />`, context copied | text only |
| list | `[a,b]` | `<list>a b</list>` | text only |
| path | **copied into the store**, emits the store path | emits its own spelling, no copy | behaviour |
| attrs | `__toString`, then `outPath`, else object | `isDerivation`, then `drvsSeen` and `<repeated/>`, else `<attrs>`; no `__toString` at all | behaviour |
| function | error, "cannot convert a function to JSON" | `<function>` with formals from the IR | behaviour |

So the renderers are genuinely two things and the shared part is the *driver*,
not the rendering. That is an argument for the split rather than against it: an
enum with a `kind` discriminant would be one `match (kind, value)` with
eighteen arms, and neither rendering would be readable end to end. A trait with
two implementations keeps each one whole, and the driver keeps the parts that
are identical.

The driver owns the worklist, `Lit` interleaving, depth and its limit, context
accumulation, name-sorted attribute order, and the `Force`/`Apply`/`Need`
suspensions with a renderer-defined resume tag. A `Box<dyn Renderer>` inside
the existing `Cont::Ext` variant costs one allocation per call and avoids two
near-identical variants.

## Decision

One driver, two renderer clients (`toJSON` migrated, `toXML` new), `deepSeq`
left alone and given its missing depth counter separately.

The driver owns the worklist, `Lit` interleaving, depth and its limit, context
accumulation, and the `Force`/`Apply`/`Need` suspensions. Each renderer supplies
the text for each value type, decides which children to queue, and may request
auxiliary forced values before emitting.

Migrating `toJSON` in the same change is the part that has to be proved rather
than asserted: the bar is that `builtins.toJSON` output is byte-identical
before and after on the lang corpus and on a real nixpkgs attribute, since it
already feeds derivation hashes.
