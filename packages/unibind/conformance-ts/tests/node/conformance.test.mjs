// End-to-end conformance for the unibind ts backend (issue #1993). Runs
// against the built npm package (generated index.js + native addon):
//
//   UNIBIND_CONFORMANCE_PKG=<package root> \
//     node --expose-gc --test --test-isolation=none conformance.test.mjs
//
// --expose-gc powers the drop-without-close case (and --test-isolation=none
// keeps the flag applied to the test code itself); `await using` needs
// Node >= 24, where explicit resource management is stable.

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

test("records echo with camelCased and renamed fields", () => {
  const facts = {
    occurrence: [
      { symbol: "sym", path: "src/lib.rs", start: 1, end: 4, occurrenceRole: "definition" },
    ],
    docsBySymbol: { sym: "does things" },
    sourceBlob: [1, 2, 255],
  };
  const echoed = api.echoFacts(facts);
  assert.deepEqual(echoed.occurrence, facts.occurrence);
  assert.deepEqual(echoed.docsBySymbol, facts.docsBySymbol);
  assert.ok(Array.isArray(echoed.sourceBlob), "nested Vec<u8> crosses as number[]");
  assert.deepEqual(Array.from(echoed.sourceBlob), facts.sourceBlob);

  const occurrences = api.makeOccurrences("sym");
  assert.equal(occurrences.length, 2, "count defaults to 2 when omitted");
  assert.ok(
    Object.hasOwn(occurrences[0], "occurrenceRole"),
    "the ts field rename reaches the JS object shape",
  );
  assert.equal(occurrences[0].role, undefined);
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

test("abort mid-flight rejects promptly and drops the Rust future", async () => {
  const baseline = api.droppedMidFlightCount();
  const controller = new AbortController();
  const started = Date.now();
  const pending = api.sleepEcho("never", 500, controller.signal);
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

test("streams exert backpressure through the bounded(2) channel", async () => {
  const baseline = api.streamItemsProduced();
  const stream = api.countStream(20);
  let consumed = 0;
  for (let pull = 0; pull < 3; pull += 1) {
    assert.equal(await stream.next(), consumed);
    consumed += 1;
    await sleep(50); // an unthrottled producer would run far ahead here
    const produced = api.streamItemsProduced() - baseline;
    assert.ok(
      produced <= consumed + 3,
      `producer pushed ${produced} with only ${consumed} consumed; bounded(2) should cap it`,
    );
  }
  stream.close();
  await sleep(100); // let a send blocked on the full channel observe the close
  const settled = api.streamItemsProduced() - baseline;
  await sleep(100);
  assert.equal(
    api.streamItemsProduced() - baseline,
    settled,
    "producer kept pushing after close()",
  );
  assert.ok(settled < 20, `producer pushed all ${settled} items despite the early close`);
  assert.equal(await stream.next(), null, "next() after close() resolves null");
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

test("objects also arrive from plain function returns", async () => {
  const baseline = api.liveSessions();
  const session = api.openSession("beta");
  assert.equal(api.liveSessions(), baseline + 1);
  assert.equal(await session.query("hi"), "beta: hi");
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
