import { svelte } from '@sveltejs/vite-plugin-svelte';
import { playwright } from '@vitest/browser-playwright';
import { mdsvex, escapeSvelte } from 'mdsvex';
import { codeToHtml } from 'shiki';
import { defineConfig } from 'vitest/config';

// vitest uses @sveltejs/vite-plugin-svelte directly (not the full sveltekit
// plugin) so the browser server doesn't try to spin up a SvelteKit dev
// server. mdsvex is wired in by hand against the same shape as
// svelte.config.js so .svx imports resolve in tests too.
export default defineConfig({
  plugins: [
    svelte({
      extensions: ['.svelte', '.svx'],
      preprocess: [
        mdsvex({
          extensions: ['.svx'],
          highlight: {
            highlighter: async (code, lang = 'text') => {
              const html = await codeToHtml(code, {
                lang,
                themes: { light: 'github-light', dark: 'github-dark' },
                defaultColor: false
              });
              return `{@html \`${escapeSvelte(html)}\`}`;
            }
          }
        })
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
