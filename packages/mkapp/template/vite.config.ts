import path from 'node:path';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  resolve: {
    alias: {
      // The vendored shadcn-svelte components import through `$lib`. Vite
      // serves the live tree, so the alias points at src/;
      // tsconfig.staging.json repoints it at staging/ for the gate's
      // typecheck.
      $lib: path.resolve(import.meta.dirname, 'src/lib'),
    },
  },
  server: {
    // Serve.app advertises http://<hostname>:<port>/ and the terminal split
    // iframes load it across the tailnet; Vite 7's DNS-rebinding guard 403s
    // any non-localhost Host header unless allowed.
    allowedHosts: true,
  },
});
