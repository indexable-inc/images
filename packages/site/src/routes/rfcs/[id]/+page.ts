import { redirect } from '@sveltejs/kit';
import { resolve } from '$app/paths';
import { planEntries } from '$lib/plans';
import type { EntryGenerator, PageLoad } from './$types';

export const prerender = true;

// One redirect stub per published /rfcs/<id> URL.
export const entries: EntryGenerator = () => planEntries();

export const load: PageLoad = ({ params }): never => redirect(301, resolve('/plans/[id]', { id: params.id }));
