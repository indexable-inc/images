#!/usr/bin/env node
// svelte-bundle: compile one Svelte 5 component into a self-contained IIFE
// bundle for the sandboxed (opaque-origin, no network) resource iframe.
//
//   svelte-bundle Entry.svelte            # bundle JS on stdout
//   svelte-bundle Entry.svelte --out f.js # write to file
//   svelte-bundle Entry.svelte --json     # {"js": ..., "warnings": [...]} on stdout
//
// The entry component is mounted on document.body. CSS is injected at runtime
// (compilerOptions.css "injected"), so the output is exactly one <script>
// payload. The virtual `ix` module (see ./ix.js) resolves to the runtime
// wiring around window.ix / window.__IX_STATE__.
import { fileURLToPath } from "node:url";
import path from "node:path";
import fs from "node:fs";
import { parseArgs } from "node:util";
import esbuild from "esbuild";
import sveltePlugin from "esbuild-svelte";

const here = path.dirname(fileURLToPath(import.meta.url));

const { values: opts, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    out: { type: "string" },
    json: { type: "boolean", default: false },
    minify: { type: "boolean", default: false },
  },
});
const entry = positionals[0];
if (!entry || positionals.length !== 1) {
  console.error("usage: svelte-bundle <Entry.svelte> [--out file] [--json] [--minify]");
  process.exit(2);
}
const entryAbs = path.resolve(entry);
if (!fs.existsSync(entryAbs)) {
  console.error(`svelte-bundle: no such file: ${entryAbs}`);
  process.exit(2);
}

// Generated wrapper entry. Svelte 5 `mount` replaces the class-component
// `new App(...)` API. Resource HTML is often scripts-only, which the parser
// executes from <head> before <body> exists, so defer the mount until the DOM
// is ready. Mount into the script's containing element when it sits in the
// body (ix-windows wraps producer HTML in a measured #ix-content div, and a
// body-mounted app would land outside it and size the window to nothing);
// scripts hoisted into <head> fall back to document.body.
const wrapper = `
  import { mount } from "svelte";
  import App from ${JSON.stringify(entryAbs)};
  const script = document.currentScript;
  const boot = () => {
    const host = script?.parentElement;
    mount(App, { target: host && document.body.contains(host) ? host : document.body });
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot);
  else boot();
`;

const ixModule = {
  name: "ix-virtual-module",
  setup(build) {
    build.onResolve({ filter: /^ix$/ }, () => ({
      path: path.join(here, "ix.js"),
    }));
  },
};

let result;
try {
  result = await esbuild.build({
    stdin: {
      contents: wrapper,
      resolveDir: path.dirname(entryAbs),
      sourcefile: "__ix_mount__.js",
      loader: "js",
    },
    bundle: true,
    write: false,
    format: "iife",
    target: "es2020",
    minify: opts.minify,
    // Resolve `svelte` (and the compiled output's svelte/internal imports)
    // against OUR locked node_modules regardless of where the entry component
    // lives; node_modules never exists next to a kernel temp file.
    nodePaths: [path.join(here, "node_modules")],
    // Resolve the `svelte` export condition first so component libraries ship
    // their uncompiled source to our compiler (per esbuild-svelte docs).
    mainFields: ["svelte", "browser", "module", "main"],
    conditions: ["svelte", "browser"],
    plugins: [
      ixModule,
      sveltePlugin({ compilerOptions: { css: "injected" } }),
    ],
    logLevel: "silent",
  });
} catch (err) {
  // esbuild already formats its errors; print them and fail loudly.
  for (const e of err.errors ?? []) {
    const loc = e.location ? `${e.location.file}:${e.location.line}:${e.location.column}: ` : "";
    console.error(`error: ${loc}${e.text}`);
  }
  if (!err.errors?.length) console.error(String(err));
  process.exit(1);
}

const js = result.outputFiles[0].text;
const warnings = result.warnings.map((w) => ({
  text: w.text,
  file: w.location?.file ?? null,
  line: w.location?.line ?? null,
}));
for (const w of warnings) {
  console.error(`warning: ${w.file ?? "?"}:${w.line ?? "?"}: ${w.text}`);
}

if (opts.json) {
  process.stdout.write(JSON.stringify({ js, warnings }));
} else if (opts.out) {
  fs.writeFileSync(opts.out, js);
} else {
  process.stdout.write(js);
}
