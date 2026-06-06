import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

const src = fileURLToPath(new URL('./src', import.meta.url));

// The MCP dashboard ships as ONE self-contained HTML. Nix builds it
// (ix.buildSvelteSite) and the aiohttp server (dashboard.py) serves the file
// directly, so there is no committed artifact and no sidecar assets. The data
// comes from the server's REST API (/api/jobs, /api/resources), polled by the
// app; the page itself is static.
export default defineConfig({
  build: { target: 'esnext' },
  // Dev-only: proxy the REST routes to a locally-running MCP server so
  // `vite dev` shows live data while iterating on the UI. No effect on the
  // production single-file build.
  server: {
    proxy: {
      '/api': { target: 'http://127.0.0.1:8765', changeOrigin: true }
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
