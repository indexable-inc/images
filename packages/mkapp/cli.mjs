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
  readdirSync,
  statSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const templateRoot = join(dirname(fileURLToPath(import.meta.url)), 'template');

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
// staging/ starts as an exact copy of src/: the agent edits staging/ only and
// the serve gate promotes it into src/ once checks pass.
cpSync(join(dest, 'src'), join(dest, 'staging'), { recursive: true });

// Files copied out of the Nix store keep its read-only mode; the scaffold
// must be editable.
const fixPerms = (path) => {
  if (statSync(path).isDirectory()) {
    chmodSync(path, 0o755);
    for (const name of readdirSync(path)) fixPerms(join(path, name));
  } else {
    chmodSync(path, 0o644);
  }
};
fixPerms(dest);

if (!existsSync(join(dest, 'package.json'))) {
  console.error(`mkapp: template copy left no package.json in ${dest}`);
  process.exit(1);
}

console.log(dest);
