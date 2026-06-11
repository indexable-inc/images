import assert from "node:assert/strict";
import { test } from "node:test";
// Run flat (the Nix checkPhase copies git-files.js next to this file).
import { countUnstagedFiles, parseStatusPaths } from "./git-files.js";

test("empty status means zero unstaged", () => {
  assert.equal(countUnstagedFiles(""), 0);
});

test("counts untracked and worktree-modified, not index-only", () => {
  const status = ["?? new.txt", " M dirty.txt", "M  staged-only.txt", "MM both.txt"].join("\n");
  // ??, " M", "MM" count; "M " (staged only) does not.
  assert.equal(countUnstagedFiles(status), 3);
});

test("parseStatusPaths extracts plain paths", () => {
  const paths = parseStatusPaths(["?? a.txt", " M dir/b.txt"].join("\n"));
  assert.deepEqual([...paths].sort(), ["a.txt", "dir/b.txt"]);
});

test("parseStatusPaths takes rename destination and unquotes", () => {
  const paths = parseStatusPaths(['R  old.txt -> new.txt', '?? "with space.txt"'].join("\n"));
  assert.ok(paths.has("new.txt"));
  assert.ok(paths.has("with space.txt"));
  assert.ok(!paths.has("old.txt"));
});
