import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

const src = fileURLToPath(new URL('./src', import.meta.url));

// The dashboard ships as ONE self-contained HTML embedded into the
// tui-dashboard-core Rust crate via include_str! and served by both the
// standalone aggregator and the in-process tui.serve(). viteSingleFile inlines
// the JS and CSS so there is a single artifact with no sidecar assets.
//
// loro-crdt stays a runtime import from esm.sh (it is WASM-backed; inlining it
// would bloat the file and complicate the build). Mark https imports external
// so rollup leaves them as bare module imports in the inlined script.
export default defineConfig({
  build: {
    target: 'esnext',
    rollupOptions: {
      external: [/^https:\/\//]
    }
  },
  resolve: {
    alias: {
      $lib: `${src}/lib`,
      $components: `${src}/components`
    }
  },
  plugins: [svelte(), viteSingleFile()]
});
