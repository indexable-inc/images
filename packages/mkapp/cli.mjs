// mkapp [dir]: copy the embedded Svelte 5 + Vite template into `dir` (or a
// fresh temp directory) and mirror src/ into staging/, the tree the agent
// edits. The absolute app path on stdout is the one machine-readable output,
// so callers (Serve.app pipelines, scripts) can capture it.
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const templateRoot = join(here, 'template');

const arg = process.argv[2];
if (arg === '--help' || arg === '-h') {
  console.error('usage: mkapp [dir]');
  console.error('Scaffold a Svelte 5 + Vite app; prints the absolute app path.');
  process.exit(0);
}

let dest;
if (arg === undefined) {
  dest = mkdtempSync(join(tmpdir(), 'mkapp-'));
} else {
  dest = resolve(arg);
  mkdirSync(dest, { recursive: true });
}
if (readdirSync(dest).length > 0) {
  console.error(`mkapp: ${dest} is not empty`);
  process.exit(1);
}

cpSync(templateRoot, dest, { recursive: true });

// Files copied out of the Nix store keep its read-only mode; the scaffold
// must be editable. Before anything writes into it, so the generated theme
// below lands in a writable src/lib/.
const fixPerms = (path) => {
  if (statSync(path).isDirectory()) {
    chmodSync(path, 0o755);
    for (const name of readdirSync(path)) fixPerms(join(path, name));
  } else {
    chmodSync(path, 0o644);
  }
};
fixPerms(dest);

// The palette block is generated at package build time (default.nix) and so
// exists only inside the store output: a template copied out of a checkout is
// missing a file src/app.css imports, and scaffolding from a checkout is the
// fast path worth keeping. Regenerate it into the scaffold -- never back into
// the checkout, which stays clean -- with the same generator against the same
// ghostty palettes the build uses, so both paths emit identical CSS (#4288).
const themeCss = join(dest, 'src', 'lib', 'theme.css');
if (!existsSync(themeCss)) {
  const themes = join(here, '..', '..', 'modules', 'home', 'ghostty', 'themes');
  try {
    const { renderTheme } = await import(pathToFileURL(join(here, 'generate-theme.mjs')).href);
    writeFileSync(themeCss, renderTheme(join(themes, 'custom-light'), join(themes, 'custom-dark')));
  } catch (error) {
    console.error(`mkapp: could not generate src/lib/theme.css: ${error.message}`);
  }
}

// staging/ starts as an exact copy of src/: the agent edits staging/ only and
// the serve gate promotes it into src/ once checks pass.
cpSync(join(dest, 'src'), join(dest, 'staging'), { recursive: true });

if (!existsSync(join(dest, 'package.json'))) {
  console.error(`mkapp: template copy left no package.json in ${dest}`);
  process.exit(1);
}

// Nothing between here and the browser resolves a CSS @import: svelte-check
// ignores them, `tsc` never sees them, and vite only fails once the page asks
// for the stylesheet. A stylesheet the template references but does not ship
// therefore used to survive scaffold, typecheck and serve, and surface as a
// Vite overlay on a blank page (#4288). So walk the import graph from
// src/app.css here and refuse to emit a path to a scaffold that cannot render.
const manifest = JSON.parse(readFileSync(join(dest, 'package.json'), 'utf8'));
const declaredPackages = new Set([
  ...Object.keys(manifest.dependencies ?? {}),
  ...Object.keys(manifest.devDependencies ?? {}),
]);

// `@import 'x';`, `@import "x";` and `@import url('x');` -- the spellings CSS
// allows for a quoted specifier. An unquoted `url(x)` is legal too but nothing
// in the template writes one.
const importPattern = /@import\s+(?:url\(\s*)?['"]([^'"]+)['"]/g;

// The package a bare specifier resolves to: `pkg`, `@scope/pkg`, or a subpath
// of either.
const packageOf = (specifier) => {
  const parts = specifier.split('/');
  return specifier.startsWith('@') ? parts.slice(0, 2).join('/') : parts[0];
};

// A specifier carrying a URL scheme (https:, data:) is the browser's problem,
// not the scaffold's.
const isRemote = (specifier) => /^[a-z][a-z0-9+.-]*:/i.test(specifier);

const collectMissing = (file, missing, seen) => {
  if (seen.has(file)) return;
  seen.add(file);
  const where = relative(dest, file);
  for (const [, specifier] of readFileSync(file, 'utf8').matchAll(importPattern)) {
    if (specifier.startsWith('.')) {
      const target = resolve(dirname(file), specifier);
      if (existsSync(target) && statSync(target).isFile()) {
        collectMissing(target, missing, seen);
      } else {
        missing.push(`${where} imports '${specifier}', which the scaffold does not contain`);
      }
    } else if (isRemote(specifier)) {
      continue;
    } else if (!declaredPackages.has(packageOf(specifier))) {
      missing.push(`${where} imports '${specifier}', which package.json does not depend on`);
    }
  }
};

// Both trees, because the serve gate promotes staging/ over src/: a stylesheet
// present in one and absent in the other breaks the app on promotion instead.
const missing = [];
for (const root of ['src', 'staging']) {
  const entry = join(dest, root, 'app.css');
  if (existsSync(entry)) collectMissing(entry, missing, new Set());
  else missing.push(`${root}/app.css is missing`);
}
if (missing.length > 0) {
  console.error(`mkapp: incomplete scaffold in ${dest}, refusing to emit it:`);
  for (const line of missing) console.error(`  ${line}`);
  process.exit(1);
}

console.log(dest);
