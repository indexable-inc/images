import { mount } from 'svelte';
import Deck from './Deck.svelte';
import '../app.css';

document.documentElement.dataset.theme = window.matchMedia(
  '(prefers-color-scheme: dark)'
).matches
  ? 'dark'
  : 'light';

mount(Deck, { target: document.getElementById('app')! });
