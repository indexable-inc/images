import { mount } from 'svelte';
import App from './App.svelte';
import './style.css';

const target = document.getElementById('app');

if (!(target instanceof HTMLElement)) {
  throw new Error('Missing #app mount target');
}

const app = mount(App, { target });

export default app;
