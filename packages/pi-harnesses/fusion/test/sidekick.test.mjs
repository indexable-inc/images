import assert from "node:assert/strict";
import test from "node:test";
import { buildSidekickPrompt, countDiffLines, formatSidekickResult } from "./sidekick.js";

test("buildSidekickPrompt includes goal, task, acceptance, and mode", () => {
  const prompt = buildSidekickPrompt({
    goal: "ship fusion",
    task: "edit the harness",
    acceptance: "nix build passes",
    mode: "inspect",
  });
  assert.match(prompt, /Goal: ship fusion/);
  assert.match(prompt, /Delegated task: edit the harness/);
  assert.match(prompt, /Acceptance check or success criteria: nix build passes/);
  assert.match(prompt, /Inspection mode: do not edit files/);
});

test("countDiffLines sums text numstat rows and ignores binary markers", () => {
  assert.equal(countDiffLines("3\t2\tfoo.ts\n-\t-\timage.png\n10\t0\tbar.js\n"), 15);
});

test("formatSidekickResult includes empty patch marker", () => {
  const text = formatSidekickResult({
    exitCode: 0,
    stdout: "done",
    stderr: "",
    diffLines: 0,
    patch: "",
  });
  assert.match(text, /Sidekick completed/);
  assert.match(text, /\(no file changes produced\)/);
});
