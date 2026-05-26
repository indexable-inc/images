import { mount, tick, unmount } from 'svelte';
import { describe, expect, test } from 'vitest';
import SafetyFixture from './fixtures/mdsvex-safety.svx';

describe('mdsvex rendering', () => {
  test('keeps unsafe body links inert and falls back for unknown code languages', async () => {
    const target = document.createElement('div');
    document.body.append(target);

    const component = mount(SafetyFixture, { target });
    await tick();

    expect(link(target, 'unsafe')?.hasAttribute('href')).toBe(false);
    expect(link(target, 'protocol-relative')?.hasAttribute('href')).toBe(false);
    expect(link(target, 'root')?.getAttribute('href')).toBe('/index/notes');
    expect(link(target, 'fragment')?.getAttribute('href')).toBe('#notes');
    expect(link(target, 'absolute')?.getAttribute('href')).toBe(
      'https://example.com/a%22b'
    );
    expect(target.querySelector('pre code')?.textContent).toContain('plain fallback');

    await unmount(component);
    target.remove();
  });
});

function link(root: ParentNode, text: string): HTMLAnchorElement | undefined {
  return Array.from(root.querySelectorAll('a')).find(
    (anchor) => anchor.textContent === text
  );
}
