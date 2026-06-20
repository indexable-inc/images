import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Relative base so the built site works served from any directory (the nix
// wrapper drops it next to the run's data.json and serves the folder).
export default defineConfig({
  base: './',
  plugins: [svelte()],
});
