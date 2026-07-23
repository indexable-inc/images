import assert from "node:assert/strict";
import { test } from "node:test";
// Run flat (the Nix checkPhase copies scoring.js next to this file).
import { rank, scoreBranch } from "./scoring.js";

test("passing always beats failing regardless of diff size", () => {
  const pass = scoreBranch({ exitCode: 0, diffLines: 9999 });
  const fail = scoreBranch({ exitCode: 1, diffLines: 1 });
  assert.ok(pass > fail);
});

test("among passers, the smaller diff wins", () => {
  const small = scoreBranch({ exitCode: 0, diffLines: 10 });
  const big = scoreBranch({ exitCode: 0, diffLines: 500 });
  assert.ok(small > big);
});

test("rank sorts winner first", () => {
  const branches = [
    { approach: "fail-small", score: scoreBranch({ exitCode: 1, diffLines: 5 }) },
    { approach: "pass-big", score: scoreBranch({ exitCode: 0, diffLines: 400 }) },
    { approach: "pass-small", score: scoreBranch({ exitCode: 0, diffLines: 20 }) },
  ];
  const ordered = rank(branches).map((b) => b.approach);
  assert.deepEqual(ordered, ["pass-small", "pass-big", "fail-small"]);
});

test("missing diffLines is treated as zero", () => {
  assert.equal(scoreBranch({ exitCode: 0 }), 1e6);
});
