import { redirect } from '@sveltejs/kit';
import { resolve } from '$app/paths';

export const prerender = true;

// The collection was renamed to Plans; published /rfcs links keep working.
export const load = (): never => redirect(301, resolve('/plans'));
