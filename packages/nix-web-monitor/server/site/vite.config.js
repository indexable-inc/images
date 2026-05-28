import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';
import topLevelAwait from 'vite-plugin-top-level-await';
import wasm from 'vite-plugin-wasm';

export default defineConfig({
  build: {
    target: 'esnext'
  },
  optimizeDeps: {
    exclude: ['loro-crdt']
  },
  plugins: [svelte(), wasm(), topLevelAwait()]
});
