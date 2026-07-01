import { error } from '@sveltejs/kit';
import { findRfc, rfcEntries } from '$lib/rfcs';
import type { EntryGenerator, PageLoad } from './$types';

export const prerender = true;

export const entries: EntryGenerator = () => rfcEntries();

export const load: PageLoad = ({ params }) => {
  const rfc = findRfc(params.id);
  if (!rfc) error(404, `Unknown RFC: ${params.id}`);
  return { rfc };
};
