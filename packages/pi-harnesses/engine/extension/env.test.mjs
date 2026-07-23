import assert from "node:assert/strict";
import test from "node:test";

import { EXTRA_ALLOWLIST_ENV, buildMcpEnv } from "./env.js";

test("buildMcpEnv keeps only safe defaults", () => {
  const env = buildMcpEnv({
    PATH: "/bin",
    HOME: "/home/test",
    IX_MCP_HOST: "127.0.0.1",
    EXA_API_KEY: "not-default-safe",
    RANDOM_SECRET: "hidden",
  });

  assert.deepEqual(env, {
    HOME: "/home/test",
    PATH: "/bin",
    IX_MCP_HOST: "127.0.0.1",
  });
});

test("buildMcpEnv strips model-provider keys even when explicitly allowlisted", () => {
  const env = buildMcpEnv({
    [EXTRA_ALLOWLIST_ENV]:
      "OPENAI_API_KEY,ANTHROPIC_API_KEY,AWS_SECRET_ACCESS_KEY,EXA_API_KEY",
    OPENAI_API_KEY: "openai-secret",
    ANTHROPIC_API_KEY: "anthropic-secret",
    AWS_SECRET_ACCESS_KEY: "aws-secret",
    EXA_API_KEY: "tool-secret",
  });

  assert.deepEqual(env, {
    EXA_API_KEY: "tool-secret",
  });
});

test("buildMcpEnv ignores malformed allowlist names", () => {
  const env = buildMcpEnv({
    [EXTRA_ALLOWLIST_ENV]: "OK_NAME,NOT-AN-ENV, ALSO_OK ",
    OK_NAME: "ok",
    ALSO_OK: "also-ok",
    "NOT-AN-ENV": "bad",
  });

  assert.deepEqual(env, {
    OK_NAME: "ok",
    ALSO_OK: "also-ok",
  });
});
