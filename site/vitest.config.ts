import { fileURLToPath } from 'node:url';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { playwright } from '@vitest/browser-playwright';
import { mdsvex } from 'mdsvex';
import { defineConfig } from 'vitest/config';
import { siteMdsvexOptions } from './mdsvex.config.js';

// vitest uses @sveltejs/vite-plugin-svelte directly (not the full sveltekit
// plugin) so the browser server doesn't try to spin up a SvelteKit dev
// server. mdsvex is wired in by hand against the same shape as
// svelte.config.js so .svx imports resolve in tests too. SvelteKit's `$lib`
// alias is wired in by hand for the same reason — without it, .svx files
// that `import Foo from '$lib/...'` would fail to resolve in the browser
// test runtime.
export default defineConfig({
  resolve: {
    alias: {
      $lib: fileURLToPath(new URL('./src/lib', import.meta.url))
    }
  },
  plugins: [
    svelte({
      extensions: ['.svelte', '.svx'],
      preprocess: [
        mdsvex(siteMdsvexOptions)
      ]
    })
  ],
  test: {
    include: ['src/**/*.test.ts'],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright({
        launchOptions: {
          args: ['--no-sandbox', '--disable-dev-shm-usage']
        }
      }),
      instances: [{ browser: 'chromium' }]
    }
  }
});
