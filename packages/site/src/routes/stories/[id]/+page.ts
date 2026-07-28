import { error } from '@sveltejs/kit';
import { findStory, stories } from '$lib/stories';
import type { EntryGenerator, PageLoad } from './$types';

export const prerender = true;

export const entries: EntryGenerator = () => stories.map((story) => ({ id: story.id }));

export const load: PageLoad = ({ params }) => {
  const story = findStory(params.id);
  if (!story) error(404, `Unknown story: ${params.id}`);
  return { story };
};
