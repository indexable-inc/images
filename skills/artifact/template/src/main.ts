import { mount } from 'svelte';
import App from './App.svelte';
import './app.css';

// Resolve the theme before first paint so there is no flash; ThemeToggle
// flips the same attribute afterwards.
document.documentElement.dataset.theme = window.matchMedia(
  '(prefers-color-scheme: dark)'
).matches
  ? 'dark'
  : 'light';

mount(App, { target: document.getElementById('app')! });
