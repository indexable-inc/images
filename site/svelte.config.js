import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { mdsvex } from 'mdsvex';
import { siteMdsvexOptions } from './mdsvex.config.js';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  extensions: ['.svelte', '.svx'],
  preprocess: [
    vitePreprocess(),
    mdsvex(siteMdsvexOptions)
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
