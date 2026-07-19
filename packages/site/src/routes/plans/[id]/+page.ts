import { error } from '@sveltejs/kit';
import { findPlan, planEntries } from '$lib/plans';
import type { EntryGenerator, PageLoad } from './$types';

export const prerender = true;

export const entries: EntryGenerator = () => planEntries();

export const load: PageLoad = ({ params }) => {
  const plan = findPlan(params.id);
  if (!plan) error(404, `Unknown Plan: ${params.id}`);
  return { plan };
};
