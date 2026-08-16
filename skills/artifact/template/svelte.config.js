import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { mdsvex } from 'mdsvex';
import { mdsvexOptions } from './mdsvex.config.js';

export default {
  extensions: ['.svelte', '.svx'],
  preprocess: [vitePreprocess(), mdsvex(mdsvexOptions)]
};
