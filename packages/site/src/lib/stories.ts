import type { Component } from 'svelte';

// Case-example shorts: each answers "why would I want this repo" with one
// concrete situation, in a few sentences and a diagram. Linked from the
// README, so ids are load-bearing once published.
export type StoryMeta = {
  id: string;
  title: string;
  // One-line situation hook shown on the index and under README links.
  hook: string;
  // Listing position, ascending.
  order: number;
};

export type Story = StoryMeta & {
  component: Component;
};

type SvxModule = {
  default: Component;
  metadata: StoryMeta;
};

const modules = import.meta.glob<SvxModule>('./stories/*.svx', { eager: true });

for (const [path, mod] of Object.entries(modules)) {
  const stem = path.slice(path.lastIndexOf('/') + 1).replace(/\.svx$/, '');
  if (mod.metadata.id !== stem) {
    throw new Error(`story ${path}: frontmatter id '${mod.metadata.id}' disagrees with filename '${stem}'`);
  }
  for (const field of ['title', 'hook'] as const) {
    if (typeof mod.metadata[field] !== 'string' || mod.metadata[field].length === 0) {
      throw new Error(`story ${path}: missing '${field}'`);
    }
  }
  if (!Number.isInteger(mod.metadata.order)) {
    throw new Error(`story ${path}: 'order' must be an integer`);
  }
}

export const stories: Story[] = Object.values(modules)
  .map((mod) => ({ ...mod.metadata, component: mod.default }))
  .sort((a, b) => a.order - b.order);

export function findStory(id: string): Story | undefined {
  return stories.find((story) => story.id === id);
}
