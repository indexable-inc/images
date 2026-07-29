import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
// The update surface, imported for its side effects: its statements run against
// the store on every promote, which is how a change reaches a page the reader
// already has open.
import './lib/live';

const app = mount(App, { target: document.getElementById('app')! });

export default app;
