// End-to-end conformance for the unibind wasm backend (issue #1993). Runs
// against the built browser package (generated index.js + the
// `wasm-bindgen --target web` output under wasm/):
//
//   UNIBIND_CONFORMANCE_PKG=<package root> \
//     node --expose-gc --test --test-isolation=none conformance.test.mjs
//
// --expose-gc powers the drop-without-close cases, which are the only way to
// observe a wasm resource's teardown: there is no Drop-at-collection story in
// wasm, so the generated wrapper watches with a FinalizationRegistry and the
// suite forces the collection (--test-isolation=none keeps the flag applied to
// the test code itself). `await using` needs Node >= 24, where explicit
// resource management is stable.
//
// The node half of this surface is `packages/unibind/conformance-ts`, and the
// two suites are deliberately close readings of each other: same constructs,
// same invariants, different binding library underneath. Where an assertion
// differs, the difference is a wasm-bindgen or serde fact and says so in
// place -- bytes inside a record are serde's array of numbers rather than a
// napi Buffer, `usize` is 32 bits wide, and an absent `Option` arrives
// `undefined` from a sync export.
//
// Every integer on this surface crosses as a JavaScript `number` and never a
// `bigint`: node and the browser publish one `.d.ts` vocabulary. The glue
// refuses inbound 64-bit numbers that are fractional or outside the
// double-exact range instead of truncating them; outbound values past 2^53
// round to the nearest representable double, exactly as they would in any
// JSON API.

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const pkgRoot = process.env.UNIBIND_CONFORMANCE_PKG;
assert.ok(pkgRoot, "set UNIBIND_CONFORMANCE_PKG to the built package root");

// The generated files as text. Doc comments are part of what this package
// publishes, so how one renders is a conformance case like any other.
const generated = Object.fromEntries(
  ["index.d.ts", "index.js", "schemas.ts"].map((name) => [
    name,
    fs.readFileSync(path.join(pkgRoot, name), "utf8"),
  ]),
);

// An ES module, imported by URL: the package is an absolute path outside this
// file's own tree, which `import` only accepts as a file:// URL.
const api = await import(pathToFileURL(path.join(pkgRoot, "index.js")).href);

// Nothing in the module works before the initializer has been awaited once.
// The object form (`{ module_or_path }`) rather than a bare argument: passing
// the bytes positionally is deprecated in wasm-bindgen 0.2.123 and warns on
// `console.warn`, which the leak cases below read as their evidence.
const wasmFile = fs
  .readdirSync(path.join(pkgRoot, "wasm"))
  .find((name) => name.endsWith(".wasm"));
assert.ok(wasmFile, "the package has no .wasm under wasm/");
await api.init({
  module_or_path: fs.readFileSync(path.join(pkgRoot, "wasm", wasmFile)),
});

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function pollUntil(check, { timeoutMs = 2000, stepMs = 10 } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (check()) return true;
    if (Date.now() >= deadline) return false;
    await sleep(stepMs);
  }
}

// The collection-driven cases: a FinalizationRegistry callback runs on a later
// turn of the event loop than the collection itself, so each attempt forces a
// GC and then yields.
async function pollAfterGc(check, { timeoutMs = 10000, stepMs = 50 } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    globalThis.gc();
    await sleep(stepMs);
    if (check()) return true;
    if (Date.now() >= deadline) return false;
  }
}

// Capture what the generated wrapper writes to `console.warn` while `body`
// runs. The leak warning has no other channel: the Rust glue deliberately
// renders none (a wasm handle's free is the engine's business), so this is the
// surface under test rather than a convenience.
async function capturingWarnings(body) {
  const warnings = [];
  const original = console.warn;
  console.warn = (...args) => {
    warnings.push(args.join(" "));
  };
  try {
    await body(warnings);
  } finally {
    console.warn = original;
  }
  return warnings;
}

const occurrence = (symbol, role = "definition") => ({
  symbol,
  path: `src/${symbol}.rs`,
  // Numbers, not 1n/4n: `start`/`end` are i64, which this binding maps to JS
  // number by contract (the "a bigint where a number is declared is refused"
  // test below proves the refusal side of the same contract).
  start: 1,
  end: 4,
  occurrenceRole: role,
});

// Every non-optional field of `Facts` has to be present: the record crosses
// through a generated serde twin, and serde refuses an object missing a key
// that is not `Option`-typed.
const facts = (overrides = {}) => ({
  occurrence: [],
  docsBySymbol: {},
  sourceBlob: [],
  blobChunks: [],
  byPath: {},
  ...overrides,
});

test("records echo with camelCased and renamed fields", () => {
  const sent = facts({
    occurrence: [occurrence("sym")],
    docsBySymbol: { sym: "does things" },
    sourceBlob: [1, 2, 255],
    blobChunks: [[1, 2, 255]],
  });
  const echoed = api.echoFacts(sent);
  assert.deepEqual(echoed.occurrence, sent.occurrence);
  assert.deepEqual(echoed.docsBySymbol, sent.docsBySymbol);
  // Both byte positions, side by side. A record field is serde's, not the
  // signature's, so neither of these is the `Uint8Array` a whole argument
  // crosses as (see the bytes test below): both are plain number arrays, one
  // nested a level deeper than the other.
  assert.ok(Array.isArray(echoed.sourceBlob), "a record field of bytes crosses as an array");
  assert.ok(
    !(echoed.sourceBlob instanceof Uint8Array),
    "serde has no Buffer carve-out: a record field of bytes is not a typed array",
  );
  assert.deepEqual(echoed.sourceBlob, sent.sourceBlob);
  assert.ok(Array.isArray(echoed.blobChunks), "Vec<Vec<u8>> crosses as an array of arrays");
  assert.deepEqual(echoed.blobChunks, sent.blobChunks);

  const occurrences = api.makeOccurrences("sym");
  assert.equal(occurrences.length, 2, "count defaults to 2 when omitted");
  assert.ok(
    Object.hasOwn(occurrences[0], "occurrenceRole"),
    "the field rename reaches the JS object shape",
  );
  assert.equal(occurrences[0].role, undefined);
});

// The double-exact boundary drives every case below: inside +/-(2^53 - 1)
// integers cross exactly; outside it inbound values are refused and outbound
// values round.
const MAX_SAFE = Number.MAX_SAFE_INTEGER;

test("64-bit integers cross as exact numbers inside the safe range", () => {
  for (const value of [MAX_SAFE, -MAX_SAFE, 2 ** 52, -(2 ** 52), 0, -1]) {
    assert.equal(api.echoI64(value), value);
  }
  for (const value of [MAX_SAFE, 2 ** 52, 0]) {
    assert.equal(api.echoU64(value), value);
  }
  assert.equal(typeof api.echoI64(1), "number");

  // The widths' own endpoints, read from Rust: they exceed the safe range, so
  // they arrive rounded to the nearest double -- the same value the equivalent
  // JSON payload would parse to.
  assert.deepEqual(api.i64Bounds(), [Number(-(2n ** 63n)), Number(2n ** 63n - 1n)]);
  assert.equal(api.u64Max(), Number(2n ** 64n - 1n));

  // Defaults cross as numbers too.
  assert.equal(api.addI64(2 ** 52), 2 ** 52 + 1, "the default operand crosses as a number");
  assert.equal(api.addI64(2 ** 52, 3), 2 ** 52 + 3);
});

test("a number the declared width cannot hold exactly is refused, never truncated", () => {
  for (const [call, what] of [
    [() => api.echoU64(-1), "negative into u64"],
    [() => api.echoI64(1.5), "fractional into i64"],
    [() => api.echoI64(MAX_SAFE + 2), "past the double-exact range"],
    [() => api.echoI64(-(MAX_SAFE + 2)), "below the double-exact range"],
    [() => api.echoI64(Number.POSITIVE_INFINITY), "infinity"],
    [() => api.echoI64(Number.NaN), "NaN"],
  ]) {
    assert.throws(
      call,
      (error) => {
        assert.match(error.message, /is not a safe integer for a Rust/, what);
        assert.ok(
          !(error instanceof api.ConformanceError),
          "a caller mistake is not one of the declared boundary failures",
        );
        return true;
      },
      what,
    );
  }
  // The safe-range endpoints themselves must still cross.
  assert.equal(api.echoI64(MAX_SAFE), MAX_SAFE);
  assert.equal(api.echoI64(-MAX_SAFE), -MAX_SAFE);
});

test("usize is 32 bits wide here, and the pointer-sized bound is enforced", () => {
  // The narrowing the other backends' 64-bit hosts never exercise: wasm is a
  // 32-bit target, so `usize` stops at 2^32 - 1 and the checked conversion has
  // a real bound to enforce well below the double-exact range.
  assert.equal(api.usizeMax(), 4294967295, "usize::MAX is the 32-bit one");
  assert.equal(api.echoUsize(api.usizeMax()), 4294967295, "the top of the range still crosses");
  assert.equal(api.echoUsize(0), 0);
  for (const value of [api.usizeMax() + 1, MAX_SAFE, -1]) {
    assert.throws(
      () => api.echoUsize(value),
      /is not a safe integer for a Rust `usize`/,
      `usize accepted ${value}`,
    );
  }
});

test("a narrow width coerces where a 64-bit width refuses", () => {
  // The measured asymmetry, pinned rather than papered over: `u32` has a
  // faithful wasm-bindgen ABI, so the value crosses natively and JavaScript's
  // own ToInt32 coercion applies, while `i64` crosses through the checked
  // `f64` adaptation and is refused. Tightening the narrow widths would flip
  // this assertion, which is the point of writing it down.
  assert.equal(api.echoU32(7), 7);
  assert.equal(api.echoU32(-1), 4294967295, "a negative number wraps into u32");
  assert.equal(api.echoU32(1.5), 1, "a fractional number truncates into u32");
  assert.throws(() => api.echoI64(-1.5), /is not a safe integer/, "the 64-bit path still refuses");
});

test("a bigint where a number is declared is refused rather than coerced", () => {
  // TypeScript rejects this at compile time; the module refuses it at run time
  // too, so a plain-JavaScript caller cannot silently pass a bigint.
  assert.throws(() => api.echoI64(1n), TypeError);
  assert.throws(() => api.echoU64(1n), TypeError);
});

test("wide integers cross inside containers, records, and streams", async () => {
  const ledger = {
    balance: -(2 ** 52),
    sequence: MAX_SAFE,
    entries: 2 ** 31,
    deltas: [1, -1, 2 ** 51],
    ceiling: 2 ** 50,
    totals: { alpha: 2 ** 52, beta: 0 },
  };
  assert.deepEqual(api.echoLedger(ledger), ledger);

  assert.equal(api.sumU64([2 ** 52, 1]), 2 ** 52 + 1);
  assert.equal(api.echoOptionalI64(2 ** 52 + 5), 2 ** 52 + 5);
  assert.equal(api.echoOptionalI64(), MAX_SAFE, "the declared default fills an omitted value");

  // A wide field inside a record is checked like a whole argument.
  assert.throws(
    () => api.echoLedger({ ...ledger, entries: MAX_SAFE }),
    /is not a safe integer for a Rust `usize`/,
    "a record field's declared width is enforced too",
  );

  const start = 2 ** 52;
  const items = [];
  for await (const item of api.wideStream(start, 3)) {
    items.push(item);
  }
  assert.deepEqual(items, [start, start + 1, start + 2]);
});

test("records compose: a record under Option, and records as map values", () => {
  const head = occurrence("head");
  const byPath = { "src/head.rs": head, "src/other.rs": occurrence("other", "reference") };
  const echoed = api.echoFacts(facts({ head, byPath }));
  assert.deepEqual(echoed.head, head, "Option<Record> round-trips as the nested object");
  assert.deepEqual(echoed.byPath, byPath, "Map<String, Record> round-trips");

  // The twin serializes with `json_compatible`, which spells an absent value
  // `null` rather than leaving the key off: the same wire vocabulary the ts
  // backend's records have, so one declaration (`head?: Occurrence | null`)
  // describes both.
  const headless = api.echoFacts(facts());
  assert.equal(headless.head, null, "an omitted Option<Record> came back set");
  assert.deepEqual(headless.byPath, {}, "an empty record map stays empty");

  // The other half of the declaration: an explicit `null` is accepted inbound.
  // No normalization pass is needed for that here (serde_wasm_bindgen reads
  // `null` and `undefined` alike as `None`), which is why the browser wrapper
  // forwards arguments untouched where the node one rewrites them.
  const nulled = api.echoFacts(facts({ head: null }));
  assert.equal(nulled.head, null, "an explicit null Option field reads as unset");
  const undefinedHead = api.echoFacts(facts({ head: undefined }));
  assert.equal(undefinedHead.head, null, "an explicit undefined Option field reads as unset");
});

// The bytes fixture: 0x00, 0xFF, 0xFE and a lone 0x80 continuation byte, none
// of which survives a UTF-8 decode. Rust mints it, so a decode-then-re-encode
// anywhere on the path shows up as a comparison failure instead of passing
// quietly.
test("bytes cross as a Uint8Array wherever the signature carries them", () => {
  const fixture = api.blobFixture();
  assert.ok(fixture instanceof Uint8Array, "a whole return value of bytes is a Uint8Array");
  assert.deepEqual(Array.from(fixture), [0x00, 0xff, 0xfe, 0x69, 0x78, 0x80]);
  assert.notEqual(
    Buffer.from(Buffer.from(fixture).toString("utf8"), "utf8").compare(Buffer.from(fixture)),
    0,
    "the fixture must not be UTF-8 round-trippable, or it proves nothing",
  );

  const doubled = api.doubleBytes(fixture);
  assert.ok(doubled instanceof Uint8Array, "a whole argument of bytes crosses back as a Uint8Array");
  assert.deepEqual(Array.from(doubled), [0x00, 0xfe, 0xfc, 0xd2, 0xf0, 0x00]);
});

test("a record field of bytes takes either array shape and always answers with an array", () => {
  const fixture = api.blobFixture();
  // Inbound, serde reads any JavaScript iterable of numbers for a `Vec<u8>`
  // field, so both the declared `Array<number>` and a typed array reach Rust
  // unchanged. Outbound there is only one shape: serde makes a `Vec<u8>` an
  // array of numbers, and the declaration says so.
  for (const [payload, what] of [
    [Array.from(fixture), "the declared Array<number>"],
    [fixture, "a Uint8Array, which serde also accepts"],
  ]) {
    const echoed = api.echoBlobs({ payload });
    assert.ok(Array.isArray(echoed.payload), `${what} came back as something else`);
    assert.deepEqual(echoed.payload, Array.from(fixture), what);
    assert.equal(echoed.trailer, null, "an omitted Option<Vec<u8>> came back set");
  }

  const trailer = [0xff, 0x00];
  const both = api.echoBlobs({ payload: Array.from(fixture), trailer });
  assert.deepEqual(both.payload, Array.from(fixture));
  assert.deepEqual(both.trailer, trailer, "Option<Vec<u8>> round-trips");

  // A missing non-optional field is refused by name rather than defaulted.
  assert.throws(() => api.echoBlobs({}), /missing field `payload`/);

  // The same bytes through the bigger record, whose twin also narrows 64-bit
  // fields: the two reasons a twin exists have to compose.
  const echoed = api.echoFacts(
    facts({ sourceBlob: Array.from(fixture), blobChunks: [Array.from(fixture)] }),
  );
  assert.deepEqual(echoed.sourceBlob, Array.from(fixture));
  assert.deepEqual(echoed.blobChunks, [Array.from(fixture)]);
});

test("unit enums cross as plain strings, both ways", () => {
  // The value is a string, not a wrapper object: `JSON.stringify` round-trips
  // it and the declared union type is assignable from `JSON.parse` output.
  const echoed = api.echoSeverity("warning");
  assert.equal(echoed, "warning");
  assert.equal(typeof echoed, "string");

  // Rust matched on the variant rather than passing the string through.
  assert.equal(api.escalate("info"), "warning");
  assert.equal(api.escalate("warning"), "hard_failure");

  // `rename_all` decides the literal; this enum is PascalCase on the wire.
  assert.equal(api.echoOptionalKind("Finished"), "Finished");
});

test("unit enums cross as record fields", () => {
  const finding = { severity: "hard_failure", kind: "Started", detail: "boom" };
  const echoed = api.echoFinding(finding);
  assert.deepEqual(echoed, finding);
  assert.equal(JSON.parse(JSON.stringify(echoed)).severity, "hard_failure");
});

test("a string outside the closed set is refused by name", () => {
  // Not silently mapped to a neighbouring variant, and not passed through: the
  // message names the offending word and the set it should have come from, in
  // every position an enum can occupy.
  assert.throws(
    () => api.echoSeverity("catastrophe"),
    (error) => {
      assert.match(error.message, /catastrophe/);
      assert.match(error.message, /expected one of info, warning, hard_failure/);
      assert.ok(
        !(error instanceof api.ConformanceError),
        "a word outside a closed set is a caller mistake, not a declared failure",
      );
      return true;
    },
  );
  assert.throws(() => api.echoOptionalKind("started"), /`started` is not a FrameKind/);
  assert.throws(() => api.echoFinding({ severity: "nope", kind: "Started", detail: "x" }), /nope/);
});

test("an enum refusal from an async export arrives as a rejection", async () => {
  // The same conversion, on the other side of the sync/async split: an async
  // wrapper converts its arguments inside the future, so the refusal is the
  // rejection a throwing `async fn` would have produced.
  await assert.rejects(api.echoOptionalKindLater("nope"), /`nope` is not a FrameKind/);
});

test("an absent Option is undefined from a sync export and null from an async one", () => {
  // Measured, and pinned because a caller comparing against one of them is
  // broken by the other. A sync wrapper hands `wasm-bindgen` the `Option`
  // itself, whose `None` is `undefined`; an async wrapper settles a `JsValue`,
  // and the glue spells `None` there as `null`. Both are declared
  // `FrameKind | null` today, so the sync half of that declaration is the one
  // that does not describe what arrives.
  assert.equal(api.echoOptionalKind(null), undefined, "sync None");
  assert.equal(api.echoOptionalKind(), undefined, "sync None, argument omitted");
});

test("the async half of the same Option answers null", async () => {
  assert.equal(await api.echoOptionalKindLater(null), null, "async None");
  assert.equal(await api.echoOptionalKindLater("Started"), "Started");
});

test("errors decode to the generated classes with the variant code", () => {
  assert.throws(
    () => api.failWith("store"),
    (error) => {
      assert.ok(error instanceof api.ConformanceError, "instanceof base class");
      assert.ok(error instanceof api.StoreMissingError, "instanceof renamed variant subclass");
      assert.equal(error.code, "StoreMissingError");
      assert.equal(error.name, "StoreMissingError");
      assert.equal(error.message, "store `main` does not exist");
      return true;
    },
  );
  assert.throws(() => api.failWith("query"), api.BadQuery);
  assert.throws(() => api.failWith("anything"), api.OutOfRange);
  assert.equal(api.checkedAdd(2, 3), 5);
  assert.throws(() => api.checkedAdd(900, 200), api.OutOfRange);
});

test("sync functions substitute omitted defaults", () => {
  assert.equal(api.joinWords(["a", "b"]), "a, b", "the string default");
  assert.equal(api.joinWords(["a", "b"], "-", "x:"), "x:a-b");
  assert.equal(api.scale(10), "5.000", "the float, narrow-integer and bool defaults");
  assert.equal(api.scale(10, 0.25, 2, false), "2.50");
  assert.equal(api.echoPath("src/a/b.rs"), "src/a/b.rs", "a path crosses as a string");
});

test("async functions resolve as real promises and decode rejections", async () => {
  const pending = api.queuedEcho("hi");
  assert.ok(pending instanceof Promise, "async exports return a Promise");
  assert.equal(await pending, "hi");
  await assert.rejects(api.failLater(), (error) => {
    assert.ok(error instanceof api.BadQuery);
    assert.equal(error.code, "BadQuery");
    return true;
  });
});

// Abort in flight: the call rejects with an AbortError while its future is
// still parked on the gate, and the Rust future is dropped (its guard bumps
// droppedMidFlightCount). `start` takes the signal, so a free function and a
// method are the same measurement. There is no sleep on this target, so the
// gate -- not a clock -- is what holds the future in flight, and the abort
// races nothing.
async function assertAbortsMidFlight(start) {
  api.armPending();
  const baseline = api.droppedMidFlightCount();
  const controller = new AbortController();
  const started = Date.now();
  const pending = start(controller.signal);
  setTimeout(() => controller.abort(), 20);
  await assert.rejects(pending, (error) => {
    assert.equal(error.name, "AbortError");
    return true;
  });
  const elapsed = Date.now() - started;
  assert.ok(elapsed < 2000, `abort took ${elapsed}ms; the gate was still closed`);
  assert.ok(
    await pollUntil(() => api.droppedMidFlightCount() > baseline),
    "droppedMidFlightCount never moved: the Rust future was not dropped",
  );
  api.releasePending();
}

test("abort mid-flight rejects promptly and drops the Rust future", async () => {
  await assertAbortsMidFlight((signal) => api.pendingEcho("never", signal));
  // The same export completes when nothing aborts it, so the case above
  // measured the abort and not a call that could never resolve.
  api.armPending();
  const pending = api.pendingEcho("released");
  api.releasePending();
  assert.equal(await pending, "released");
});

test("an already-aborted signal rejects before the future starts", async () => {
  api.armPending();
  const baseline = api.droppedMidFlightCount();
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(api.pendingEcho("never", controller.signal), (error) => {
    assert.equal(error.name, "AbortError");
    return true;
  });
  // The glue reads `.aborted` before first polling the future, so the fn body
  // (and its drop guard) never runs: the counter must not move.
  await sleep(50);
  assert.equal(api.droppedMidFlightCount(), baseline);
  api.releasePending();
});

test("streams collect through for-await", async () => {
  const items = [];
  for await (const item of api.countStream(5)) {
    items.push(item);
  }
  assert.deepEqual(items, [0, 1, 2, 3, 4]);
});

test("an async stream function resolves to an iterable stream", async () => {
  const stream = await api.countStreamLater(3);
  const items = [];
  for await (const item of stream) {
    items.push(item);
  }
  assert.deepEqual(items, [0, 1, 2]);
});

// The bounded(2) pull, whatever opened the stream: the producer runs at most a
// couple of items ahead of the consumer, stops when the stream closes, and a
// closed stream's next() resolves null. There is no producer delay to lean on
// here (no time driver on wasm32), so the channel's own capacity is the whole
// throttle -- which is the stronger statement anyway: the producer is a
// detached spawn_local task with nothing but backpressure holding it back, and
// it still cannot outrun the consumer. `open` is a factory so the
// produced-counter baseline is read before anything is produced.
async function assertBoundedPull({ open, produced, item, total }) {
  const baseline = produced();
  const stream = open();
  let consumed = 0;
  for (let pull = 0; pull < 3; pull += 1) {
    assert.equal(await stream.next(), item(consumed));
    consumed += 1;
    await sleep(50); // an unthrottled producer would run to `total` here
    const ahead = produced() - baseline;
    assert.ok(
      ahead <= consumed + 3,
      `producer pushed ${ahead} with only ${consumed} consumed; bounded(2) should cap it`,
    );
  }
  stream.close();
  await sleep(100); // let a send blocked on the full channel observe the close
  const settled = produced() - baseline;
  await sleep(100);
  assert.equal(produced() - baseline, settled, "producer kept pushing after close()");
  assert.ok(settled < total, `producer pushed all ${settled} items despite the early close`);
  assert.equal(await stream.next(), null, "next() after close() resolves null");
}

test("streams exert backpressure through the bounded(2) channel", async () => {
  await assertBoundedPull({
    open: () => api.countStream(20),
    produced: () => api.streamItemsProduced(),
    item: (index) => index,
    total: 20,
  });
});

test("early break from for-await closes the stream", async () => {
  const baseline = api.streamItemsProduced();
  const collected = [];
  for await (const item of api.countStream(50)) {
    collected.push(item);
    if (collected.length === 2) break;
  }
  assert.deepEqual(collected, [0, 1]);
  await sleep(100);
  const settled = api.streamItemsProduced() - baseline;
  await sleep(100);
  assert.equal(api.streamItemsProduced() - baseline, settled, "producer survived the break");
  assert.ok(settled < 50, `producer pushed all ${settled} items despite the break`);
});

test("a stream ends with null rather than hanging", async () => {
  const stream = api.countStream(2);
  assert.equal(await stream.next(), 0);
  assert.equal(await stream.next(), 1);
  assert.equal(await stream.next(), null, "the exhausted stream resolves null");
  assert.equal(await stream.next(), null, "and keeps resolving null");
});

test("objects construct, expose methods, and close idempotently", async () => {
  assert.throws(() => new api.Session(""), api.BadQuery, "constructor errors decode");
  const liveBaseline = api.liveSessions();
  const closedBaseline = api.closedSessions();
  const session = new api.Session("alpha");
  assert.equal(api.liveSessions(), liveBaseline + 1);
  assert.equal(session.name(), "alpha");
  assert.equal(session.isOpen(), true);
  assert.equal(await session.query("ping"), "alpha: ping");
  await session.close();
  assert.equal(api.closedSessions(), closedBaseline + 1, "close ran the Rust close");
  assert.equal(session.isOpen(), false, "methods still answer after close");
  await session.close();
  assert.equal(api.closedSessions(), closedBaseline + 1, "second close is a no-op");
});

test("associated functions: construct the object, and answer about it", async () => {
  const liveBaseline = api.liveSessions();

  // The shape a constructor cannot take: a static that awaits before it has an
  // instance to hand back.
  const opened = await api.Session.opened("gamma");
  assert.ok(opened instanceof api.Session, "async factory returns the wrapper class");
  assert.equal(opened.name(), "gamma");
  assert.equal(api.liveSessions(), liveBaseline + 1);

  // A second factory on the same object, sync this time.
  const copy = api.Session.namedAfter("gamma");
  assert.ok(copy instanceof api.Session, "sync factory returns the wrapper class");
  assert.equal(copy.name(), "gamma-copy");

  // A factory's errors decode like any other call's.
  await assert.rejects(api.Session.opened(""), api.BadQuery, "async factory errors decode");
  assert.throws(() => api.Session.namedAfter(""), api.BadQuery, "sync factory errors decode");

  // The instance a factory returns is a real resource: `await using` closes
  // it, which is the whole point of returning the wrapper rather than a bare
  // handle.
  const closedBaseline = api.closedSessions();
  {
    await using scoped = await api.Session.opened("delta");
    assert.equal(scoped.name(), "delta");
  }
  assert.equal(
    api.closedSessions(),
    closedBaseline + 1,
    "await using closed the factory's instance",
  );

  // An associated function that does not return the object hands its value
  // straight back instead of wrapping it in the class.
  const badge = api.Session.describe("epsilon");
  assert.equal(badge.label, "session:epsilon");
  assert.ok(!(badge instanceof api.Session), "a record return is not wrapped as the class");

  await opened.close();
  await copy.close();
});

test("objects also arrive from plain function returns", async () => {
  const baseline = api.liveSessions();
  const session = api.openSession("beta");
  assert.equal(api.liveSessions(), baseline + 1);
  assert.equal(await session.query("hi"), "beta: hi");
  await session.close();
});

test("a stream method iterates, and its items carry the receiver's state", async () => {
  const session = api.openSession("streamy");
  const items = [];
  for await (const item of session.events(4)) {
    items.push(item);
  }
  assert.deepEqual(items, ["streamy:0", "streamy:1", "streamy:2", "streamy:3"]);
  await session.close();
});

test("a stream method exerts backpressure and stops at close()", async () => {
  const session = api.openSession("bounded");
  await assertBoundedPull({
    open: () => session.events(20),
    produced: () => api.sessionEventsProduced(),
    item: (index) => `bounded:${index}`,
    total: 20,
  });
  await session.close();
});

test("a throwing stream method fails before any stream exists", async () => {
  const session = api.openSession("guarded");
  assert.throws(
    () => session.tail(""),
    (error) => {
      assert.ok(error instanceof api.BadQuery, "the method's rejection decodes to its class");
      assert.equal(error.code, "BadQuery");
      return true;
    },
  );
  const lines = [];
  for await (const line of session.tail("q")) {
    lines.push(line);
  }
  assert.deepEqual(lines, ["guarded/q#0", "guarded/q#1", "guarded/q#2"]);
  await session.close();
});

test("aborting an async stream method rejects and drops the Rust future", async () => {
  const session = api.openSession("abortable");
  await assertAbortsMidFlight((signal) => session.eventsLater(3, signal));
  const items = [];
  api.armPending();
  const pending = session.eventsLater(3);
  api.releasePending();
  for await (const item of await pending) {
    items.push(item);
  }
  assert.deepEqual(items, [0, 1, 2], "the same method still streams when it is not aborted");
  await session.close();
});

test("a stream method yields records whose 64-bit fields cross as checked numbers", async () => {
  const session = api.openSession("ledger");
  // Inside the double-exact range: an inbound value past 2^53 - 1 is refused
  // by the integer policy, so a wide inbound value is not a scenario.
  const start = 2 ** 52;
  const rows = [];
  for await (const row of session.ledgers(start, 2)) {
    rows.push(row);
  }
  assert.equal(rows.length, 2);
  // The owner-scoped stream class and the record's generated twin have to
  // compose: every field arrives as a plain number.
  assert.deepEqual(
    rows.map((row) => row.balance),
    [start, start + 1],
  );
  // Outbound values past 2^53 round exactly as they would in any JSON API.
  assert.equal(rows[0].sequence, Number(2n ** 64n - 1n), "u64::MAX rounds across the stream element");
  assert.equal(rows[1].entries, 1, "usize crosses as a number inside a streamed record");
  assert.deepEqual(rows[1].deltas, [1], "a Vec<i64> field inside a streamed record");
  assert.equal(rows[0].ceiling, start, "an Option<i64> field inside a streamed record");
  assert.deepEqual(rows[0].totals, { ledger: Number(2n ** 64n - 1n) }, "a u64-valued map field");
  await session.close();
});

test("a method returns a record, sync and after an await", async () => {
  const session = api.openSession("badged");
  assert.deepEqual(session.badge(), { label: "badged" });
  assert.deepEqual(await session.badgeLater(), { label: "badged!" });
  await session.close();
});

test("a method returning another object hands back the wrapper class", async () => {
  const session = api.openSession("keyed");
  const keys = session.keys();
  assert.ok(keys instanceof api.Keys, "the returned handle is the generated wrapper class");
  assert.equal(keys.create("signing"), "keyed/signing");
  assert.throws(
    () => keys.reject("nope"),
    (error) => {
      assert.ok(error instanceof api.BadQuery, "errors decode through the returned object too");
      return true;
    },
  );
  assert.throws(() => new api.Keys(), TypeError, "Keys instances only come from Session.keys()");
  await session.close();
});

test("an async fallible method hands back the wrapper class, not a native handle", async () => {
  const session = api.openSession("shells");
  const closedBaseline = api.closedShells();

  const pending = session.openShell("bash");
  assert.ok(pending instanceof Promise, "an async method returns a Promise");
  const shell = await pending;

  // The generated wrapper class, not the raw wasm-bindgen handle: a bare
  // handle would carry no error decoding and no disposal, and `instanceof` is
  // the only thing that tells them apart from JavaScript.
  assert.ok(shell instanceof api.Shell, "the awaited value is the generated wrapper class");
  assert.equal(shell.constructor.name, "Shell");
  assert.equal(shell.command(), "shells/bash");
  assert.equal(shell.isOpen(), true);
  await shell.close();
  assert.equal(shell.isOpen(), false);
  assert.equal(api.closedShells(), closedBaseline + 1, "the minted object's close never ran");

  // Instances only come from the method, exactly like Keys.
  assert.throws(() => new api.Shell(), TypeError);

  // The error arm of the same shape still rejects with the decoded class
  // rather than resolving to a half-built handle.
  await assert.rejects(session.openShell(""), (error) => {
    assert.ok(error instanceof api.BadQuery, "the rejection decodes to its generated class");
    assert.equal(error.code, "BadQuery");
    return true;
  });
  await session.close();
});

test("a bytes stream method yields Uint8Arrays with every byte intact", async () => {
  const session = api.openSession("blobs");
  const shell = await session.openShell("cat");
  const chunks = [];
  for await (const chunk of shell.output(3)) {
    chunks.push(chunk);
  }
  assert.equal(chunks.length, 3);
  for (const chunk of chunks) {
    // A stream item is a whole value, so bytes there are the signature's and
    // cross as a typed array -- not the number array a record field carries.
    assert.ok(chunk instanceof Uint8Array, `stream item is not a Uint8Array: ${typeof chunk}`);
  }
  // 0x00 and 0xFF are both invalid on their own in UTF-8 text, so a lossy
  // string conversion anywhere on the item path cannot reproduce these.
  const expected = [0, 1, 2].map((index) =>
    Uint8Array.from([0x00, 0xff, ...Buffer.from(`blobs/cat${index}`, "latin1")]),
  );
  assert.deepEqual(chunks, expected);
  assert.equal(chunks[0][0], 0x00, "the leading NUL survived");
  assert.equal(chunks[0][1], 0xff, "the high byte survived");
  await shell.close();
  await session.close();
});

test("await using disposes the session through its Rust close", async () => {
  const closedBaseline = api.closedSessions();
  {
    await using session = new api.Session("scoped");
    assert.equal(session.name(), "scoped");
    assert.equal(api.closedSessions(), closedBaseline);
  }
  assert.equal(api.closedSessions(), closedBaseline + 1, "asyncDispose did not run the Rust close");
});

// The two collection cases, in this order on purpose: the silence case runs
// first, so a warning left over from the leak case below cannot be mistaken
// for its own. Both drain the collector before they start measuring, since
// every earlier test's closed-but-uncollected wrapper is also waiting to be
// swept (a close unregisters the watch, so those are silent by construction --
// which is exactly what the first case proves).
test("closing a resource silences the leak warning", async () => {
  await pollAfterGc(() => false, { timeoutMs: 300, stepMs: 50 });
  const warnings = await capturingWarnings(async () => {
    await (async () => {
      const session = api.openSession("closed-then-dropped");
      await session.close();
    })();
    await pollAfterGc(() => false, { timeoutMs: 1500, stepMs: 50 });
  });
  assert.deepEqual(warnings, [], "a closed resource must not warn when it is collected");
});

test("drop without close: the wrapper warns and the Rust value is freed", async () => {
  assert.equal(typeof globalThis.gc, "function", "run with --expose-gc");
  const baseline = api.liveSessions();
  const warnings = await capturingWarnings(async (collected) => {
    (() => {
      const session = api.openSession("leaked");
      assert.equal(session.name(), "leaked");
    })();
    assert.equal(api.liveSessions(), baseline + 1);
    // Two registries have to run for this to settle: the generated wrapper's,
    // which prints the warning, and wasm-bindgen's own, which frees the
    // handle and so drops the Rust value. `<=`: forcing GC here also sweeps
    // earlier tests' closed-but-alive sessions (close runs the Rust close;
    // only the free drops the value), so the count can fall below this test's
    // baseline.
    const settled = await pollAfterGc(
      () => collected.length > 0 && api.liveSessions() <= baseline,
    );
    assert.ok(
      settled,
      `collection never settled (warnings: ${JSON.stringify(collected)}, ` +
        `live delta: ${api.liveSessions() - baseline})`,
    );
  });
  assert.deepEqual(
    warnings,
    ["unclosed Session: call close() or use `await using`"],
    "the generated FinalizationRegistry did not name the unclosed resource",
  );
});

test("the module publishes its initializer under both spellings", () => {
  // A browser package is useless until the wasm module is instantiated, so the
  // wrapper re-exports wasm-bindgen's own initializer as both `default` and
  // `init`. This suite already depends on it working; the assertion is that
  // both names reach the same function.
  assert.equal(typeof api.init, "function");
  assert.equal(typeof api.default, "function");
  assert.equal(api.init, api.default, "init and default are the same initializer");
});

// A rustdoc intra-doc link in a Rust doc comment is resolved against the
// interface and re-spelled the way JavaScript spells the item it names, so
// `[`Session::events`]` publishes as {@link Session.events} and an editor can
// follow it. Each case pins one target kind at one doc site; the string is what
// the generated file has to contain verbatim.
const renderedLinks = [
  ["{@link Finding} carries one.", "a record type, from an enumeration's own docs"],
  [
    '- `warning`: Worth a look; {@link escalate} promotes `"info"` to this.',
    "an exported function, and a sibling variant as its wire literal, from a variant's docs",
  ],
  [
    '- `hard_failure`: Stop now: {@link escalate} leaves `"warning"` here.',
    "a second variant's docs, resolved through `Self::`",
  ],
  [
    "{@link Occurrence.occurrenceRole} renders as.",
    "a record field carrying a rename, from that field's own docs",
  ],
  [
    "{@link Facts.sourceBlob}, chunked.",
    "a camelCased record field, from a sibling field's docs",
  ],
  ["one of {@link Severity}'s literals", "an enumeration type, from a record field's docs"],
  ['`"hard_failure"`.', "an enumeration variant as its wire literal, from a record field's docs"],
  [
    "{@link FrameKind} is `PascalCase` on the",
    "an enumeration whose variants are renamed wholesale",
  ],
  ['wire, so `"Started"` keeps its capital.', "that enumeration's variant, in its wire spelling"],
  ["see {@link usizeMax}.", "an exported function, from a record field's docs"],
  ["{@link failWith} mints one", "a camelCased exported function, from an error enum's own docs"],
  ["{@link Session.tail} raises it for an", "an object method, from an error variant's docs"],
  ["{@link checkedAdd} refuses.", "a camelCased function, from a second error variant's docs"],
  [
    "{@link Session.namedAfter}). An empty name raises",
    "a sibling associated function through `Self::`, camelCased",
  ],
  ["{@link BadQuery}.", "an error variant, from an associated function's docs"],
  ["Returns a {@link Badge}, not the object", "a record type, from an associated function"],
  [
    "instance method {@link Session.badge}",
    "an object method through `Self::`, from an associated function",
  ],
  ["The {@link Shell} it hands back is the generated", "an object type, from an object method"],
  ["{@link Facts} survives.", "the inline `[text](Target)` form, whose link text is dropped"],
  ['{@link StoreMissingError}, `"query"` for', "an error variant carrying a rename"],
  ["{@link OutOfRange}.", "a third error variant, from the same function's docs"],
  [
    "{@link Session.opened} is the associated-function path",
    "an associated function as a target, from an exported function's docs",
  ],
  [
    "{@link armPending} /",
    "an exported function, from the exported module's own docs",
  ],
  [
    "{@link droppedMidFlightCount} observes.",
    "an exported function, from an async export's docs",
  ],
];

test("intra-doc links reach index.d.ts in JavaScript's own spelling", () => {
  for (const [rendered, what] of renderedLinks) {
    assert.ok(
      generated["index.d.ts"].includes(rendered),
      `index.d.ts is missing ${what}: ${rendered}`,
    );
  }
});

test("the same rendering reaches index.js, where the doc block has a runtime home", () => {
  // Records and enumerations are types only, so their docs stop at the
  // declarations; everything with a class or a function behind it carries the
  // same rendered block into the runtime file too.
  for (const rendered of [
    "{@link failWith} mints one",
    "{@link Session.tail} raises it for an",
    "{@link Session.namedAfter}). An empty name raises",
    "instance method {@link Session.badge}",
    "The {@link Shell} it hands back is the generated",
    "{@link Facts} survives.",
    '{@link StoreMissingError}, `"query"` for',
    "{@link Session.opened} is the associated-function path",
  ]) {
    assert.ok(generated["index.js"].includes(rendered), `index.js is missing: ${rendered}`);
  }
});

test("no rustdoc link syntax survives into the generated files", () => {
  // The load-bearing one. Every assertion above names a link that exists
  // today, so together they cannot notice a link nobody thought to list -- and
  // a link that ships unresolved is exactly the failure this mechanism exists
  // to make impossible. Any `[`...`]` left anywhere in a published file fails
  // here, whoever wrote it and whenever it appeared.
  for (const [name, text] of Object.entries(generated)) {
    const leftover = text.split("\n").filter((line) => line.includes("[`"));
    assert.deepEqual(leftover, [], `${name} publishes unresolved rustdoc link syntax`);
  }
});
