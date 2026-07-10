import { error } from '@sveltejs/kit';
import { siteUpdates } from '$lib/updates';
import type { EntryGenerator, PageLoad } from './$types';

export const prerender = true;

export const entries: EntryGenerator = () =>
  siteUpdates.map((update) => ({ id: update.id }));

export const load: PageLoad = ({ params }) => {
  const update = siteUpdates.find((u) => u.id === params.id);
  if (!update) error(404, `Unknown update: ${params.id}`);
  return { update };
};
