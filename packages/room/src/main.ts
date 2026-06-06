import { mount } from 'svelte';
import App from './App.svelte';
// Variable Fira Code (single woff2 covers the full weight range). The
// @fontsource-variable package ships the font files locally so we
// don't depend on a CDN. CSS rules in app.css enable the ligatures.
import '@fontsource-variable/fira-code';
import './app.css';
import { startActivityTracking } from '$lib/activity';

const target = document.getElementById('app');

if (!(target instanceof HTMLElement)) {
  throw new Error('Missing #app mount target');
}

startActivityTracking();

const app = mount(App, { target });

export default app;
