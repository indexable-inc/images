#!/usr/bin/env node
"use strict";

// Launcher shim installed as the wrapper package's `bin` entry by
// `ix.buildNpmDist` (lib/build/npm-dist.nix). Resolves the platform package
// that npm selected from the wrapper's `optionalDependencies` (one package
// per `os`/`cpu` pair, the esbuild/turborepo distribution pattern) and runs
// the real compiled binary, forwarding argv, stdio, the exit code, and a
// fatal signal. The file is fully static: everything CLI-specific (the
// command name, the platform -> package map) is read from the wrapper's own
// package.json `npmDist` field, so one shim serves every CLI the builder
// packages.

const path = require("node:path");
const { spawnSync } = require("node:child_process");

const manifest = require(path.join(__dirname, "..", "package.json"));
const dist = manifest.npmDist;

const fail = (message) => {
  process.stderr.write(`${manifest.name}: ${message}\n`);
  process.exit(1);
};

const platformKey = `${process.platform}-${process.arch}`;
const platformPackage = dist.platforms[platformKey];

if (!platformPackage) {
  fail(
    `unsupported platform ${platformKey}; supported platforms: ${Object.keys(dist.platforms).sort().join(", ")}`,
  );
}

// Windows binaries ship (and must be spawned) with the `.exe` suffix.
const executable = process.platform === "win32" ? `${dist.binName}.exe` : dist.binName;

// The platform packages carry no `exports` field, so subpath resolution
// reaches the raw binary file. `require.resolve` (rather than a hand-built
// node_modules path) follows whatever layout the installer produced:
// hoisted, nested, or workspace-linked.
let binaryPath;
try {
  binaryPath = require.resolve(`${platformPackage}/bin/${executable}`);
} catch {
  fail(
    `platform package ${platformPackage} is not installed. It ships as an optionalDependency, ` +
      "so it is skipped when optional dependencies are disabled (npm install --omit=optional). " +
      `Reinstall ${manifest.name} with optional dependencies enabled.`,
  );
}

const result = spawnSync(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
  windowsHide: true,
});

if (result.error) {
  fail(result.error.message);
}

if (result.signal) {
  // Re-raise the child's fatal signal so callers observe the same
  // signal-death (128+N) they would from running the binary directly.
  process.kill(process.pid, result.signal);
}

process.exit(result.status === null ? 1 : result.status);
