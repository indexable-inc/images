import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [svelte()],
  server: {
    // The terminal split-view iframe loads this dev server from other tailnet
    // machines, and Vite's DNS-rebinding guard 403s any non-localhost Host
    // header unless hosts are allowed (index#4019).
    allowedHosts: true,
  },
});
