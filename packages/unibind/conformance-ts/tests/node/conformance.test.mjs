// End-to-end conformance for the unibind ts backend (issue #1993). Runs
// against the built npm package (generated index.js + native addon):
//
//   UNIBIND_CONFORMANCE_PKG=<package root> \
//     node --expose-gc --test --test-isolation=none conformance.test.mjs
//
// --expose-gc powers the drop-without-close case (and --test-isolation=none
// keeps the flag applied to the test code itself); `await using` needs
// Node >= 24, where explicit resource management is stable.
//
// Every integer on this surface crosses as a JavaScript `number` (the
// Stripe/OpenAI policy: records stay plain JSON). The glue refuses inbound
// numbers that are fractional or outside the double-exact range instead of
// truncating them; outbound values past 2^53 round to the nearest
// representable double, exactly as they would in any JSON API.

import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";

const pkgRoot = process.env.UNIBIND_CONFORMANCE_PKG;
assert.ok(pkgRoot, "set UNIBIND_CONFORMANCE_PKG to the built package root");
const require = createRequire(import.meta.url);
const api = require(path.join(pkgRoot, "index.js"));

// The generated files as text. Doc comments are part of what this package
// publishes, so how one renders is a conformance case like any other.
const generated = Object.fromEntries(
  ["index.d.ts", "index.js", "schemas.ts"].map((name) => [
    name,
    fs.readFileSync(path.join(pkgRoot, name), "utf8"),
  ]),
);

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function pollUntil(check, { timeoutMs = 2000, stepMs = 10 } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (check()) return true;
    if (Date.now() >= deadline) return false;
    await sleep(stepMs);
  }
}

const occurrence = (symbol, role = "definition") => ({
  symbol,
  path: `src/${symbol}.rs`,
  // Numbers, not 1n/4n: `start`/`end` are i64, which this binding maps to
  // JS number by contract (the "a bigint where a number is declared is
  // refused" test below proves the refusal side of the same contract).
  start: 1,
  end: 4,
  occurrenceRole: role,
});

test("records echo with camelCased and renamed fields", () => {
  const facts = {
    occurrence: [occurrence("sym")],
    docsBySymbol: { sym: "does things" },
    sourceBlob: Buffer.from([1, 2, 255]),
    blobChunks: [[1, 2, 255]],
    byPath: {},
  };
  const echoed = api.echoFacts(facts);
  assert.deepEqual(echoed.occurrence, facts.occurrence);
  assert.deepEqual(echoed.docsBySymbol, facts.docsBySymbol);
  assert.ok(Buffer.isBuffer(echoed.sourceBlob), "a record field of bytes crosses as a Buffer");
  assert.deepEqual(echoed.sourceBlob, facts.sourceBlob);
  // The other byte position, side by side with it: inside a container the
  // element is the user's own Vec<u8>, which crosses as a plain number
  // array. Nothing here should have turned into a Buffer.
  assert.ok(Array.isArray(echoed.blobChunks), "Vec<Vec<u8>> crosses as an array");
  assert.ok(
    !Buffer.isBuffer(echoed.blobChunks[0]),
    "bytes inside a container must not become a Buffer",
  );
  assert.deepEqual(echoed.blobChunks, facts.blobChunks);

  const occurrences = api.makeOccurrences("sym");
  assert.equal(occurrences.length, 2, "count defaults to 2 when omitted");
  assert.ok(
    Object.hasOwn(occurrences[0], "occurrenceRole"),
    "the ts field rename reaches the JS object shape",
  );
  assert.equal(occurrences[0].role, undefined);
});

// The double-exact boundary drives every case below: inside +/-(2^53 - 1)
// integers cross exactly; outside it inbound values are refused and
// outbound values round.
const MAX_SAFE = Number.MAX_SAFE_INTEGER;

test("64-bit integers cross as exact numbers inside the safe range", () => {
  for (const value of [MAX_SAFE, -MAX_SAFE, 2 ** 52, -(2 ** 52), 0, -1]) {
    assert.equal(api.echoI64(value), value);
  }
  for (const value of [MAX_SAFE, 2 ** 52, 0]) {
    assert.equal(api.echoU64(value), value);
  }
  assert.equal(api.echoUsize(MAX_SAFE), MAX_SAFE);
  assert.equal(typeof api.echoI64(1), "number");

  // The widths' own endpoints, read from Rust: they exceed the safe range,
  // so they arrive rounded to the nearest double -- the same value the
  // equivalent JSON payload would parse to.
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
        assert.match(error.message, /not a safe integer/, what);
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

test("a bigint where a number is declared is refused rather than coerced", () => {
  // TypeScript rejects this at compile time; the addon refuses it at run
  // time too, so a plain-JavaScript caller cannot silently pass a bigint.
  assert.throws(() => api.echoI64(1n));
  assert.throws(() => api.echoU64(1n));
});

test("wide integers cross inside containers, records, and streams", async () => {
  const ledger = {
    balance: -(2 ** 52),
    sequence: MAX_SAFE,
    entries: 2 ** 52 + 1,
    deltas: [1, -1, 2 ** 51],
    ceiling: 2 ** 50,
    totals: { alpha: 2 ** 52, beta: 0 },
  };
  assert.deepEqual(api.echoLedger(ledger), ledger);

  assert.equal(api.sumU64([2 ** 52, 1]), 2 ** 52 + 1);
  assert.equal(api.echoOptionalI64(2 ** 52 + 5), 2 ** 52 + 5);
  assert.equal(api.echoOptionalI64(), MAX_SAFE, "the declared default fills an omitted value");

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
  const echoed = api.echoFacts({
    occurrence: [],
    docsBySymbol: {},
    sourceBlob: Buffer.alloc(0),
    blobChunks: [],
    head,
    byPath,
  });
  assert.deepEqual(echoed.head, head, "Option<Record> round-trips as the nested object");
  assert.deepEqual(echoed.byPath, byPath, "Map<String, Record> round-trips");

  const headless = api.echoFacts({
    occurrence: [],
    docsBySymbol: {},
    sourceBlob: Buffer.alloc(0),
    blobChunks: [],
    byPath: {},
  });
  // napi returns `None` as undefined, not as an explicit null, which is why
  // `index.d.ts` declares the field optional (`head?:`) and not merely
  // nullable.
  assert.equal(headless.head, undefined, "an omitted Option<Record> came back set");
  assert.deepEqual(headless.byPath, {}, "an empty record map stays empty");

  // The other half of `head?: Occurrence | null`: an explicit `null` IS
  // accepted inbound, exactly as the declaration promises. The native layer
  // reads absence only from `undefined` (napi reads Option-typed object
  // fields with `Object::get`), so the generated wrapper normalizes `null`
  // away before any argument reaches the addon; without that pass this
  // exact call was refused with "Failed to get property names of given
  // object".
  const nulled = api.echoFacts({
    occurrence: [],
    docsBySymbol: {},
    sourceBlob: Buffer.alloc(0),
    blobChunks: [],
    head: null,
    byPath: {},
  });
  assert.equal(nulled.head, undefined, "an explicit null Option field reads as unset");
});

// The record field that carries streamed command output. `Array<number>`
// would still round-trip the bytes, so the assertions below pin both halves
// of what changed: the type (a real Buffer, usable anywhere bytes are) and
// the fidelity (0x00, 0xFF, 0xFE and a lone 0x80 continuation byte, none of
// which survives a UTF-8 decode).
test("a record field of bytes is a Buffer and survives byte for byte", () => {
  const fixture = api.blobFixture();
  assert.ok(Buffer.isBuffer(fixture), "top-level bytes cross as a Buffer");
  assert.deepEqual(Array.from(fixture), [0x00, 0xff, 0xfe, 0x69, 0x78, 0x80]);
  // Rust minted these, so a decode-then-re-encode anywhere on the path
  // would have replaced them with U+FFFD by now.
  assert.notEqual(
    Buffer.from(fixture.toString("utf8"), "utf8").compare(fixture),
    0,
    "the fixture must not be UTF-8 round-trippable, or it proves nothing",
  );

  // Outbound: Rust's Vec<u8> field arrives as a Buffer. `trailer` is
  // omitted rather than passed as an explicit `null`: napi reads an
  // Option-typed object field through `Object::get`, which treats only
  // `undefined` as absent and hands a literal `null` to the field type's
  // own conversion, where it fails. That holds for every Option field, not
  // just bytes (see the `head` case below).
  const echoed = api.echoBlobs({ payload: fixture });
  assert.ok(Buffer.isBuffer(echoed.payload), "Blobs.payload is a Buffer");
  assert.equal(echoed.payload.compare(fixture), 0, "every byte survived");
  assert.equal(echoed.trailer, undefined, "an omitted Option<Vec<u8>> came back set");

  // Inbound: a Buffer JavaScript built reaches Rust unchanged, including
  // the optional field.
  const trailer = Buffer.from([0xff, 0x00]);
  const both = api.echoBlobs({ payload: fixture, trailer });
  assert.equal(both.payload.compare(fixture), 0);
  assert.ok(Buffer.isBuffer(both.trailer), "Option<Vec<u8>> is an optional Buffer");
  assert.equal(both.trailer.compare(trailer), 0);

  // The same bytes through the bigger record, which is mirrored for its
  // 64-bit fields: the two reasons to mirror have to compose.
  const facts = api.echoFacts({
    occurrence: [],
    docsBySymbol: {},
    sourceBlob: fixture,
    blobChunks: [Array.from(fixture)],
    byPath: {},
  });
  assert.equal(facts.sourceBlob.compare(fixture), 0);
  assert.deepEqual(facts.blobChunks, [Array.from(fixture)]);
});

test("a record field of bytes refuses a plain number array", () => {
  // The `.d.ts` says `Buffer`, and the addon agrees at run time: a caller
  // who kept the old `Array<number>` shape gets an error, not a silent
  // reinterpretation.
  assert.throws(() => api.echoBlobs({ payload: [1, 2, 3], trailer: null }));
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
  assert.equal(api.echoOptionalKind(null), null);
});

test("unit enums cross as record fields", () => {
  const finding = { severity: "hard_failure", kind: "Started", detail: "boom" };
  const echoed = api.echoFinding(finding);
  assert.deepEqual(echoed, finding);
  assert.equal(JSON.parse(JSON.stringify(echoed)).severity, "hard_failure");
});

test("a string outside the closed set is refused by name", () => {
  // Not silently mapped to a neighbouring variant, and not passed through:
  // the message names the offending word and the set it should have come
  // from, in every position an enum can occupy.
  assert.throws(
    () => api.echoSeverity("catastrophe"),
    (error) => {
      assert.match(error.message, /catastrophe/);
      assert.match(error.message, /hard_failure/);
      return true;
    },
  );
  assert.throws(() => api.echoOptionalKind("started"), /started/);
  assert.throws(
    () => api.echoFinding({ severity: "nope", kind: "Started", detail: "x" }),
    /nope/,
  );
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
  assert.equal(api.joinWords(["a", "b"]), "a, b");
  assert.equal(api.joinWords(["a", "b"], "-", "x:"), "x:a-b");
  const doubled = api.doubleBytes(Buffer.from([1, 2, 3]));
  assert.ok(Buffer.isBuffer(doubled), "top-level bytes cross back as Buffer");
  assert.deepEqual(Array.from(doubled), [2, 4, 6]);
});

test("async functions resolve as real promises and decode rejections", async () => {
  const pending = api.sleepEcho("hi", 10);
  assert.ok(pending instanceof Promise, "async exports return a Promise");
  assert.equal(await pending, "hi");
  await assert.rejects(api.sleepFail(1), (error) => {
    assert.ok(error instanceof api.BadQuery);
    assert.equal(error.code, "BadQuery");
    return true;
  });
});

// Abort in flight: the call rejects with an AbortError well before its own
// 500ms sleep would resolve, and the Rust future is dropped (its guard bumps
// droppedMidFlightCount). `start` takes the signal, so a free function and a
// method are the same measurement.
async function assertAbortsMidFlight(start) {
  const baseline = api.droppedMidFlightCount();
  const controller = new AbortController();
  const started = Date.now();
  const pending = start(controller.signal);
  setTimeout(() => controller.abort(), 50);
  await assert.rejects(pending, (error) => {
    assert.equal(error.name, "AbortError");
    return true;
  });
  const elapsed = Date.now() - started;
  assert.ok(elapsed < 300, `abort took ${elapsed}ms; expected well under the 500ms sleep`);
  assert.ok(
    await pollUntil(() => api.droppedMidFlightCount() > baseline),
    "droppedMidFlightCount never moved: the Rust future was not dropped",
  );
}

test("abort mid-flight rejects promptly and drops the Rust future", async () => {
  await assertAbortsMidFlight((signal) => api.sleepEcho("never", 500, signal));
});

test("an already-aborted signal rejects before the future starts", async () => {
  const baseline = api.droppedMidFlightCount();
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(api.sleepEcho("never", 500, controller.signal), (error) => {
    assert.equal(error.name, "AbortError");
    return true;
  });
  // The glue reads `.aborted` before first polling the future, so the fn
  // body (and its drop guard) never runs: the counter must not move.
  await sleep(50);
  assert.equal(api.droppedMidFlightCount(), baseline);
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

// The bounded(2) pull, whatever opened the stream: the producer runs at
// most a couple of items ahead of the consumer, stops when the stream
// closes, and a closed stream's next() resolves null. `open` is a factory
// so the produced-counter baseline is read before anything is produced.
async function assertBoundedPull({ open, produced, item, total }) {
  const baseline = produced();
  const stream = open();
  let consumed = 0;
  for (let pull = 0; pull < 3; pull += 1) {
    assert.equal(await stream.next(), item(consumed));
    consumed += 1;
    await sleep(50); // an unthrottled producer would run far ahead here
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

  // The shape a constructor cannot take: a static that awaits before it
  // has an instance to hand back.
  const opened = await api.Session.opened("gamma");
  assert.ok(opened instanceof api.Session, "async factory returns the wrapper class");
  assert.equal(opened.name(), "gamma");
  assert.equal(api.liveSessions(), liveBaseline + 1);

  // A second factory on the same object, sync this time.
  const copy = api.Session.namedAfter("gamma");
  assert.ok(copy instanceof api.Session, "sync factory returns the wrapper class");
  assert.equal(copy.name(), "gamma-copy");

  // A factory's errors decode like any other call's.
  await assert.rejects(() => api.Session.opened(""), api.BadQuery, "async factory errors decode");
  assert.throws(() => api.Session.namedAfter(""), api.BadQuery, "sync factory errors decode");

  // The instance a factory returns is a real resource: `await using`
  // closes it, which is the whole point of returning the wrapper rather
  // than a bare handle.
  const closedBaseline = api.closedSessions();
  {
    await using scoped = await api.Session.opened("delta");
    assert.equal(scoped.name(), "delta");
  }
  assert.equal(api.closedSessions(), closedBaseline + 1, "await using closed the factory's instance");

  // An associated function that does not return the object renders as a
  // plain static, not a napi factory.
  const badge = api.Session.describe("epsilon");
  assert.equal(badge.label, "session:epsilon");
  assert.ok(!(badge instanceof api.Session), "a record return is not wrapped as the class");

  await opened.close();
  await copy.close();
});

test("objects also arrive from plain function returns", async () => {
  const baseline = api.liveSessions();
  const session = api.openSession("beta");
  // A plain number, not 1n: the counters are declared i64, which this
  // binding maps to JS number by contract (see "a bigint where a number is
  // declared is refused rather than coerced"). `+ 1n` on a number baseline
  // throws before the assertion can run.
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
  await assertAbortsMidFlight((signal) => session.eventsLater(3, 500, signal));
  const items = [];
  for await (const item of await session.eventsLater(3, 1)) {
    items.push(item);
  }
  assert.deepEqual(items, [0, 1, 2], "the same method still streams when it is not aborted");
  await session.close();
});

test("a stream method yields records whose 64-bit fields cross as checked numbers", async () => {
  const session = api.openSession("ledger");
  // Inside the double-exact range: an inbound value past 2^53 - 1 is
  // refused by the integer policy ("not a safe integer"), so the wide
  // inbound of the old bigint contract is not a scenario anymore.
  const start = 2 ** 52;
  const rows = [];
  for await (const row of session.ledgers(start, 2)) {
    rows.push(row);
  }
  assert.equal(rows.length, 2);
  // The owner-scoped stream class and the record's generated mirror have
  // to compose: every field arrives as a plain number.
  assert.deepEqual(
    rows.map((row) => row.balance),
    [start, start + 1],
  );
  // Outbound values past 2^53 round exactly as they would in any JSON
  // API; u64::MAX rounds to the nearest double, same as `u64Max()` above.
  assert.equal(rows[0].sequence, Number(2n ** 64n - 1n), "u64::MAX rounds across the stream element");
  assert.deepEqual(rows[1].entries, 1, "usize crosses as a number inside a streamed record");
  assert.deepEqual(rows[1].deltas, [1], "a Vec<i64> field inside a streamed record");
  assert.equal(rows[0].ceiling, start, "an Option<i64> field inside a streamed record");
  assert.deepEqual(rows[0].totals, { ledger: Number(2n ** 64n - 1n) }, "a u64-valued map field");
  await session.close();
});

test("a method returns a record, sync and after an await", async () => {
  const session = api.openSession("badged");
  // The shape that could not build before the glue stopped writing
  // `super::`: a record return puts a path into the exported module inside
  // the impl napi-derive relocates.
  assert.deepEqual(session.badge(), { label: "badged" });
  assert.deepEqual(await session.badgeLater(), { label: "badged!" });
  await session.close();
});

test("a method returning another object hands back the wrapper class", async () => {
  const session = api.openSession("alpha");
  const keys = session.keys();
  assert.ok(keys instanceof api.Keys, "the returned handle is the generated wrapper class");
  assert.equal(keys.create("signing"), "alpha/signing");
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

  // The generated wrapper class, not the raw napi handle: a bare handle
  // would carry no error decoding and no disposal, and `instanceof` is the
  // only thing that tells them apart from JavaScript.
  assert.ok(shell instanceof api.Shell, "the awaited value is the generated wrapper class");
  assert.equal(shell.constructor.name, "Shell");
  // Methods on the awaited handle actually answer.
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

test("a bytes stream method yields Buffers with every byte intact", async () => {
  const session = api.openSession("blobs");
  const shell = await session.openShell("cat");
  const chunks = [];
  for await (const chunk of shell.output(3)) {
    chunks.push(chunk);
  }
  assert.equal(chunks.length, 3);
  for (const chunk of chunks) {
    // Top-level bytes cross as Buffer; a `Vec<u8>` that arrived as
    // Array<number> (what nested bytes do) fails here.
    assert.ok(Buffer.isBuffer(chunk), `stream item is not a Buffer: ${typeof chunk}`);
  }
  // 0x00 and 0xFF are both invalid on their own in UTF-8 text, so a lossy
  // string conversion anywhere on the item path cannot reproduce these.
  const expected = [0, 1, 2].map((index) =>
    Buffer.concat([Buffer.from([0x00, 0xff]), Buffer.from(`blobs/cat${index}`, "latin1")]),
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
  assert.equal(
    api.closedSessions(),
    closedBaseline + 1,
    "asyncDispose did not run the Rust close",
  );
});

test("drop without close: GC finalization drops the Rust value", async () => {
  assert.equal(typeof globalThis.gc, "function", "run with --expose-gc");
  const baseline = api.liveSessions();
  let wrapperCollected = false;
  const registry = new FinalizationRegistry(() => {
    wrapperCollected = true;
  });
  (() => {
    const session = api.openSession("leaked");
    registry.register(session, "session");
  })();
  // A plain number, not 1n: the counters are declared i64, which this
  // binding maps to JS number by contract (see "a bigint where a number is
  // declared is refused rather than coerced"). `+ 1n` on a number baseline
  // throws before the assertion can run.
  assert.equal(api.liveSessions(), baseline + 1);
  // The unclosed-resource drop also prints the generated leak warning to
  // stderr, which is exactly the surface being proven here.
  // `<=`: forcing GC here also sweeps earlier tests' closed-but-alive
  // sessions (close runs the Rust close; only Drop frees the value), so
  // the count can fall below this test's baseline.
  const dropped = await pollUntil(
    () => {
      globalThis.gc();
      return wrapperCollected && api.liveSessions() <= baseline;
    },
    { timeoutMs: 10000, stepMs: 50 },
  );
  assert.ok(
    dropped,
    `napi finalizer never dropped the Rust value ` +
      `(wrapper collected: ${wrapperCollected}, live delta: ${api.liveSessions() - baseline})`,
  );
});

// A rustdoc intra-doc link in a Rust doc comment is resolved against the
// interface and re-spelled the way TypeScript spells the item it names, so
// `[`Session::events`]` publishes as {@link Session.events} and an editor can
// follow it. Each case pins one target kind at one doc site; the string is
// what the generated file has to contain verbatim.
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
    "a record field carrying a ts rename, from that field's own docs",
  ],
  [
    "The whole of {@link Facts.sourceBlob}, chunked.",
    "a camelCased record field, from a sibling field's docs",
  ],
  ["one of {@link Severity}'s literals", "an enumeration type, from a record field's docs"],
  ['`"hard_failure"`.', "an enumeration variant as its wire literal, from a record field's docs"],
  [
    "{@link FrameKind} is `PascalCase` on the",
    "an enumeration whose variants are renamed wholesale",
  ],
  ['wire, so `"Started"` keeps its capital.', "that enumeration's variant, in its wire spelling"],
  ["{@link failWith} mints one", "a camelCased exported function, from an error enum's own docs"],
  ["{@link Session.tail} raises it for an", "an object method, from an error variant's docs"],
  ["{@link checkedAdd} refuses.", "a camelCased function, from a second error variant's docs"],
  [
    "{@link Session.namedAfter}). An empty name raises",
    "a sibling associated function through `Self::`, camelCased",
  ],
  ["{@link BadQuery}.", "an error variant, from an associated function's docs"],
  ["Returns a {@link Badge}, not the object, so", "a record type, from an associated function"],
  [
    "instance method {@link Session.badge} answers the same question.",
    "an object method through `Self::`, from an associated function",
  ],
  ["The {@link Shell} it hands back is the generated", "an object type, from an object method"],
  ["{@link Facts} survives.", "the inline `[text](Target)` form, whose link text is dropped"],
  ['{@link StoreMissingError}, `"query"` for', "an error variant carrying a ts rename"],
  ["{@link OutOfRange}.", "a third error variant, from the same function's docs"],
  [
    "{@link Session.opened} is the associated-function path",
    "an associated function as a target, from an exported function's docs",
  ],
];

test("intra-doc links reach index.d.ts in TypeScript's own spelling", () => {
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
    "instance method {@link Session.badge} answers the same question.",
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
  // today, so together they cannot notice a link nobody thought to list --
  // and a link that ships unresolved is exactly the failure this mechanism
  // exists to make impossible (ENG-12396: fourteen of them outlived a rename
  // as dead text in the published .d.ts). Any `[`...`]` left anywhere in a
  // published file fails here, whoever wrote it and whenever it appeared.
  for (const [name, text] of Object.entries(generated)) {
    const leftover = text.split("\n").filter((line) => line.includes("[`"));
    assert.deepEqual(leftover, [], `${name} publishes unresolved rustdoc link syntax`);
  }
});
