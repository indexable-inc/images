import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';
import Icons from 'unplugin-icons/vite';
import wasm from 'vite-plugin-wasm';
import topLevelAwait from 'vite-plugin-top-level-await';

// Backend that the desktop client talks to. In dev, Vite proxies
// `/api` and `/ws` to this URL so the Svelte code can keep using
// relative paths. In production (`tauri build`), the bundled webview
// loads from `tauri://localhost`, so the same Svelte code reads
// VITE_ROOM_BACKEND_URL via src/lib/api.ts to know where to point.
const roomBackendUrl = process.env.ROOM_BACKEND_URL ?? 'http://127.0.0.1:8080';

// Tauri expects the dev server on a known port. 1420 is the
// convention from `create-tauri-app`; the Tauri process reads it
// from tauri.conf.json (build.devUrl).
const tauriDevPort = 1420;
const tauriHmrHost = process.env.TAURI_DEV_HOST;

const r = (p: string) => fileURLToPath(new URL(p, import.meta.url));

export default defineConfig({
  plugins: [
    svelte(),
    // loro-crdt uses `import * as wasm from './loro_wasm_bg.wasm'`,
    // which Vite's built-in handling does not support. The wasm plugin
    // turns that import into a synchronous module of exports; the
    // top-level-await plugin lets the resulting async init survive
    // Vite's lower esbuild target.
    wasm(),
    topLevelAwait(),
    Icons({
      compiler: 'svelte',
      defaultClass: 'icon',
      defaultStyle: 'display:inline-flex;vertical-align:middle'
    })
  ],
  resolve: {
    alias: {
      $lib: r('./src/lib'),
      $components: r('./src/components'),
      $routes: r('./src/routes')
    }
  },
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  // Tauri controls the webview version, so we can target modern JS
  // without polyfills. Required for top-level await + destructuring
  // emitted by vite-plugin-top-level-await.
  build: {
    target: 'esnext',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG
  },
  // Vite's esbuild dep-prebundling mangles loro-crdt's wasm import
  // (it inlines the .wasm as JS bytes that fail WebAssembly.Module).
  // Excluding it makes Vite serve the package as-is so vite-plugin-wasm
  // can do the actual instantiation. The inner `loro_wasm` entry must
  // be excluded too — esbuild scans transitive imports and would
  // otherwise try to prebundle the wasm helper directly, which is what
  // generates the recurring "504 Outdated Optimize Dep" on
  // loro_wasm.js whenever the dep graph shifts.
  optimizeDeps: {
    exclude: ['loro-crdt', 'loro-crdt/bundler/loro_wasm']
  },
  server: {
    port: tauriDevPort,
    strictPort: true,
    host: tauriHmrHost ?? false,
    hmr: tauriHmrHost
      ? { protocol: 'ws', host: tauriHmrHost, port: 1421 }
      : undefined,
    watch: {
      ignored: ['**/src-tauri/**']
    },
    // Pre-resolve the loro-crdt module on dev-server start so its wasm
    // helper is in the graph before the browser asks for it. Without
    // this, the first source-graph mutation that touches src/lib/loro.ts
    // (a new importer, a Svelte HMR boundary, a fast edit-save-edit) can
    // re-trigger esbuild dep discovery; the prebundle hash flips and any
    // already-loaded module that referenced the old URL fails with
    // "504 Outdated Optimize Dep" on loro_wasm.js until a hard reload.
    warmup: {
      clientFiles: ['./src/main.ts', './src/lib/loro.ts']
    },
    proxy: {
      '/ws': {
        target: roomBackendUrl,
        ws: true,
        changeOrigin: true
      },
      '/api': {
        target: roomBackendUrl,
        changeOrigin: true
      }
    }
  }
});
