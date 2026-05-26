import { describe, expect, test } from 'vitest';
import { renderBlock, renderInline } from './markdown';

describe('renderBlock', () => {
  test('renders paragraphs and inline code', () => {
    const html = renderBlock('A line with `code` in it.');
    expect(html).toContain('<p>A line with <code>code</code> in it.</p>');
  });

  test('strips raw HTML so script tags do not reach the DOM', () => {
    const html = renderBlock('Hello <script>alert(1)</script> world.');
    expect(html).not.toMatch(/<script/i);
    expect(html).toContain('Hello');
    expect(html).toContain('world.');
  });

  test('drops standalone raw HTML blocks entirely', () => {
    const html = renderBlock('<img src=x onerror=alert(1)>');
    expect(html).not.toMatch(/<img/i);
    expect(html).not.toMatch(/onerror/i);
  });

  test('downgrades unsafe link hrefs to plain text', () => {
    const html = renderBlock('[click](javascript:alert(1))');
    expect(html).not.toMatch(/javascript:/i);
    expect(html).toContain('click');
    expect(html).not.toMatch(/<a /);
  });

  test('preserves safe http/https/mailto/anchor link hrefs', () => {
    const cases = [
      'https://example.com',
      'http://example.com',
      'mailto:hello@example.com',
      '#section',
      '/relative'
    ];
    for (const href of cases) {
      const html = renderBlock(`[label](${href})`);
      expect(html).toContain(`href="${href}"`);
    }
  });
});

describe('renderInline', () => {
  test('renders backticks as inline code without wrapping in a paragraph', () => {
    const html = renderInline('`nix run .#site` previews locally');
    expect(html.startsWith('<code>nix run .#site</code>')).toBe(true);
    expect(html).not.toMatch(/<p>/);
  });
});
