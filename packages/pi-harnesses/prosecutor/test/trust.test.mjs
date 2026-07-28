import assert from "node:assert/strict";
import { test } from "node:test";
// Run flat (the Nix checkPhase copies trust.js next to this file).
import { createTrust } from "./trust.js";

test("starts tight", () => {
  const t = createTrust();
  assert.equal(t.interval, 1);
  assert.equal(t.streak, 0);
});

test("upheld claims double the interval up to the ceiling", () => {
  const t = createTrust({ min: 1, max: 16 });
  assert.equal(t.record(true).interval, 2);
  assert.equal(t.record(true).interval, 4);
  assert.equal(t.record(true).interval, 8);
  assert.equal(t.record(true).interval, 16);
  // ceiling holds
  assert.equal(t.record(true).interval, 16);
  assert.equal(t.streak, 5);
});

test("a broken claim snaps supervision back to every action", () => {
  const t = createTrust({ min: 1, max: 16 });
  t.record(true);
  t.record(true); // interval now 4
  const r = t.record(false);
  assert.equal(r.interval, 1);
  assert.equal(r.streak, 0);
});

test("custom min is respected", () => {
  const t = createTrust({ min: 2, max: 8 });
  assert.equal(t.interval, 2);
  assert.equal(t.record(true).interval, 4);
  assert.equal(t.record(false).interval, 2);
});
