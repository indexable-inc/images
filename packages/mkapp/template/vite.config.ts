import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [svelte()],
  server: {
    // Serve.app advertises http://<hostname>:<port>/ and the terminal split
    // iframes load it across the tailnet; Vite 7's DNS-rebinding guard 403s
    // any non-localhost Host header unless allowed.
    allowedHosts: true,
  },
});
