import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

const src = fileURLToPath(new URL('./src', import.meta.url));

export default defineConfig({
  build: {
    target: 'esnext'
  },
  resolve: {
    alias: {
      $lib: `${src}/lib`,
      $components: `${src}/components`
    }
  },
  plugins: [svelte()]
});
