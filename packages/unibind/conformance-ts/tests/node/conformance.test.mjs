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
// Every 64-bit integer on this surface crosses as a `bigint`, so the
// literals below carry the `n` suffix wherever the Rust side declares
// `i64`, `u64`, or `usize`.

import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";

const pkgRoot = process.env.UNIBIND_CONFORMANCE_PKG;
assert.ok(pkgRoot, "set UNIBIND_CONFORMANCE_PKG to the built package root");
const require = createRequire(import.meta.url);
const api = require(path.join(pkgRoot, "index.js"));

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
  start: 1n,
  end: 4n,
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

// Every value below 2^53 that JavaScript can also hold as a `number` is
// deliberately avoided in these four tests: the whole point of the bigint
// mapping is the range where a `number` silently rounds.
const TWO_53 = 2n ** 53n;

test("64-bit integers round-trip exactly past 2^53", () => {
  for (const value of [TWO_53 + 1n, 2n ** 62n, -(2n ** 62n), 0n, -1n]) {
    assert.equal(api.echoI64(value), value);
  }
  for (const value of [TWO_53 + 1n, 2n ** 64n - 1n, 0n]) {
    assert.equal(api.echoU64(value), value);
  }
  assert.equal(api.echoUsize(TWO_53 + 1n), TWO_53 + 1n);
  assert.equal(typeof api.echoI64(1n), "bigint");

  // The widths' own endpoints, read from Rust rather than restated here.
  assert.deepEqual(api.i64Bounds(), [-(2n ** 63n), 2n ** 63n - 1n]);
  assert.equal(api.u64Max(), 2n ** 64n - 1n);

  // The arithmetic a `number` boundary gets wrong: 2^53 + 1 is not
  // representable as a double, so a lossy boundary answers 2^53 here.
  assert.equal(api.addI64(TWO_53), TWO_53 + 1n, "the default operand also crosses as a bigint");
  assert.equal(api.addI64(TWO_53, 3n), TWO_53 + 3n);
});

test("a bigint outside its declared width is refused, never truncated", () => {
  for (const [call, what] of [
    [() => api.echoU64(-1n), "negative into u64"],
    [() => api.echoU64(2n ** 64n), "one past u64::MAX"],
    [() => api.echoI64(2n ** 63n), "one past i64::MAX"],
    [() => api.echoI64(-(2n ** 63n) - 1n), "one below i64::MIN"],
  ]) {
    assert.throws(
      call,
      (error) => {
        assert.match(error.message, /does not fit/, what);
        assert.ok(
          !(error instanceof api.ConformanceError),
          "a caller mistake is not one of the declared boundary failures",
        );
        return true;
      },
      what,
    );
  }
  // The endpoints themselves are in range and must still cross.
  assert.equal(api.echoI64(-(2n ** 63n)), -(2n ** 63n));
  assert.equal(api.echoI64(2n ** 63n - 1n), 2n ** 63n - 1n);
});

test("a number where a bigint is declared is refused rather than coerced", () => {
  // TypeScript rejects this at compile time; the addon refuses it at run
  // time too, so a plain-JavaScript caller cannot silently pass a double.
  assert.throws(() => api.echoI64(1));
  assert.throws(() => api.echoU64(1));
});

test("wide integers cross inside containers, records, and streams", async () => {
  const ledger = {
    balance: -(2n ** 62n),
    sequence: 2n ** 64n - 1n,
    entries: TWO_53 + 1n,
    deltas: [1n, -1n, 2n ** 61n],
    ceiling: 2n ** 60n,
    totals: { alpha: 2n ** 63n, beta: 0n },
  };
  assert.deepEqual(api.echoLedger(ledger), ledger);

  assert.equal(api.sumU64([TWO_53 + 1n, 1n]), TWO_53 + 2n);
  assert.equal(api.echoOptionalI64(TWO_53 + 5n), TWO_53 + 5n);
  assert.equal(api.echoOptionalI64(), TWO_53 + 1n, "the declared default fills an omitted value");

  const start = 2n ** 62n;
  const items = [];
  for await (const item of api.wideStream(start, 3n)) {
    items.push(item);
  }
  assert.deepEqual(items, [start, start + 1n, start + 2n]);
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

  // The other half of `head?: Occurrence | null`: an explicit `null` is
  // NOT accepted inbound. napi reads an Option-typed object field with
  // `Object::get`, which reports absence only for `undefined` and passes a
  // literal `null` down to the field type's own conversion, which refuses
  // it. This holds for every Option field regardless of its type, so the
  // `| null` half of the declaration is inbound-inaccurate; it is pinned
  // here so the behaviour is a known wart rather than a surprise.
  assert.throws(
    () => api.echoFacts({
      occurrence: [],
      docsBySymbol: {},
      sourceBlob: Buffer.alloc(0),
      blobChunks: [],
      head: null,
      byPath: {},
    }),
    "an explicit null for an Option record field is refused, not read as None",
  );
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
  assert.equal(api.checkedAdd(2n, 3n), 5n);
  assert.throws(() => api.checkedAdd(900n, 200n), api.OutOfRange);
});

test("sync functions substitute omitted defaults", () => {
  assert.equal(api.joinWords(["a", "b"]), "a, b");
  assert.equal(api.joinWords(["a", "b"], "-", "x:"), "x:a-b");
  const doubled = api.doubleBytes(Buffer.from([1, 2, 3]));
  assert.ok(Buffer.isBuffer(doubled), "top-level bytes cross back as Buffer");
  assert.deepEqual(Array.from(doubled), [2, 4, 6]);
});

test("async functions resolve as real promises and decode rejections", async () => {
  const pending = api.sleepEcho("hi", 10n);
  assert.ok(pending instanceof Promise, "async exports return a Promise");
  assert.equal(await pending, "hi");
  await assert.rejects(api.sleepFail(1n), (error) => {
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
  await assertAbortsMidFlight((signal) => api.sleepEcho("never", 500n, signal));
});

test("an already-aborted signal rejects before the future starts", async () => {
  const baseline = api.droppedMidFlightCount();
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(api.sleepEcho("never", 500n, controller.signal), (error) => {
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
  for await (const item of api.countStream(5n)) {
    items.push(item);
  }
  assert.deepEqual(items, [0n, 1n, 2n, 3n, 4n]);
});

test("an async stream function resolves to an iterable stream", async () => {
  const stream = await api.countStreamLater(3n);
  const items = [];
  for await (const item of stream) {
    items.push(item);
  }
  assert.deepEqual(items, [0n, 1n, 2n]);
});

// The bounded(2) pull, whatever opened the stream: the producer runs at
// most a couple of items ahead of the consumer, stops when the stream
// closes, and a closed stream's next() resolves null. `open` is a factory
// so the produced-counter baseline is read before anything is produced.
async function assertBoundedPull({ open, produced, item, total }) {
  const baseline = produced();
  const stream = open();
  let consumed = 0n;
  for (let pull = 0; pull < 3; pull += 1) {
    assert.equal(await stream.next(), item(consumed));
    consumed += 1n;
    await sleep(50); // an unthrottled producer would run far ahead here
    const ahead = produced() - baseline;
    assert.ok(
      ahead <= consumed + 3n,
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
    open: () => api.countStream(20n),
    produced: () => api.streamItemsProduced(),
    item: (index) => index,
    total: 20n,
  });
});

test("early break from for-await closes the stream", async () => {
  const baseline = api.streamItemsProduced();
  const collected = [];
  for await (const item of api.countStream(50n)) {
    collected.push(item);
    if (collected.length === 2) break;
  }
  assert.deepEqual(collected, [0n, 1n]);
  await sleep(100);
  const settled = api.streamItemsProduced() - baseline;
  await sleep(100);
  assert.equal(api.streamItemsProduced() - baseline, settled, "producer survived the break");
  assert.ok(settled < 50n, `producer pushed all ${settled} items despite the break`);
});

test("objects construct, expose methods, and close idempotently", async () => {
  assert.throws(() => new api.Session(""), api.BadQuery, "constructor errors decode");
  const liveBaseline = api.liveSessions();
  const closedBaseline = api.closedSessions();
  const session = new api.Session("alpha");
  assert.equal(api.liveSessions(), liveBaseline + 1n);
  assert.equal(session.name(), "alpha");
  assert.equal(session.isOpen(), true);
  assert.equal(await session.query("ping"), "alpha: ping");
  await session.close();
  assert.equal(api.closedSessions(), closedBaseline + 1n, "close ran the Rust close");
  assert.equal(session.isOpen(), false, "methods still answer after close");
  await session.close();
  assert.equal(api.closedSessions(), closedBaseline + 1n, "second close is a no-op");
});

test("objects also arrive from plain function returns", async () => {
  const baseline = api.liveSessions();
  const session = api.openSession("beta");
  assert.equal(api.liveSessions(), baseline + 1n);
  assert.equal(await session.query("hi"), "beta: hi");
  await session.close();
});

test("a stream method iterates, and its items carry the receiver's state", async () => {
  const session = api.openSession("streamy");
  const items = [];
  for await (const item of session.events(4n)) {
    items.push(item);
  }
  assert.deepEqual(items, ["streamy:0", "streamy:1", "streamy:2", "streamy:3"]);
  await session.close();
});

test("a stream method exerts backpressure and stops at close()", async () => {
  const session = api.openSession("bounded");
  await assertBoundedPull({
    open: () => session.events(20n),
    produced: () => api.sessionEventsProduced(),
    item: (index) => `bounded:${index}`,
    total: 20n,
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
  await assertAbortsMidFlight((signal) => session.eventsLater(3n, 500n, signal));
  const items = [];
  for await (const item of await session.eventsLater(3n, 1n)) {
    items.push(item);
  }
  assert.deepEqual(items, [0n, 1n, 2n], "the same method still streams when it is not aborted");
  await session.close();
});

test("a stream method yields records whose 64-bit fields cross as bigint", async () => {
  const session = api.openSession("ledger");
  const start = 2n ** 62n;
  const rows = [];
  for await (const row of session.ledgers(start, 2n)) {
    rows.push(row);
  }
  assert.equal(rows.length, 2);
  // The owner-scoped stream class and the record's generated mirror have to
  // compose: every wide field arrives as an exact bigint, past 2^53.
  assert.deepEqual(
    rows.map((row) => row.balance),
    [start, start + 1n],
  );
  assert.equal(rows[0].sequence, 2n ** 64n - 1n, "u64::MAX survived the stream element");
  assert.deepEqual(rows[1].entries, 1n, "usize crosses as bigint inside a streamed record");
  assert.deepEqual(rows[1].deltas, [1n], "a Vec<i64> field inside a streamed record");
  assert.equal(rows[0].ceiling, start, "an Option<i64> field inside a streamed record");
  assert.deepEqual(rows[0].totals, { ledger: 2n ** 64n - 1n }, "a u64-valued map field");
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
  assert.equal(api.closedShells(), closedBaseline + 1n, "the minted object's close never ran");

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
  for await (const chunk of shell.output(3n)) {
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
    closedBaseline + 1n,
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
  assert.equal(api.liveSessions(), baseline + 1n);
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
