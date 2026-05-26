import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

const roomBackendUrl = process.env.ROOM_BACKEND_URL ?? 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/ws': {
        target: roomBackendUrl,
        ws: true
      }
    }
  }
});
