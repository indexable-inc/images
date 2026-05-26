import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { mdsvex, escapeSvelte } from 'mdsvex';
import { codeToHtml } from 'shiki';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  extensions: ['.svelte', '.svx'],
  preprocess: [
    vitePreprocess(),
    mdsvex({
      extensions: ['.svx'],
      highlight: {
        // Shiki dual-theme: each span carries both light and dark colors as
        // CSS variables; app.css picks one based on prefers-color-scheme.
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
  ],
  kit: {
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      fallback: '404.html',
      precompress: false,
      strict: true
    }),
    // Site is served at https://indexable-inc.github.io/index/, so every
    // emitted URL needs the `/index` prefix. Override with BASE_PATH="" for
    // user.github.io-style deployments or a custom domain.
    paths: {
      base: process.env.BASE_PATH ?? '/index'
    }
  }
};

export default config;
