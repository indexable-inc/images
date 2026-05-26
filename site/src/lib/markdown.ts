import { Marked } from 'marked';

const safeHrefPattern = /^(https?:|mailto:|#|\/)/i;

export const renderer = new Marked({
  gfm: true,
  breaks: false,
  renderer: {
    html: () => '',
    link({ href, title, tokens }) {
      const text = this.parser.parseInline(tokens);
      if (!safeHrefPattern.test(href)) return text;
      const titleAttr = title ? ` title="${title.replace(/"/g, '&quot;')}"` : '';
      return `<a href="${href}"${titleAttr}>${text}</a>`;
    }
  }
});

export function renderBlock(markdown: string): string {
  return renderer.parse(markdown) as string;
}

export function renderInline(markdown: string): string {
  return renderer.parseInline(markdown) as string;
}
