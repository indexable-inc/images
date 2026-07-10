import { escapeSvelte } from 'mdsvex';
import { codeToHtml } from 'shiki';

const shikiThemes = { light: 'github-light', dark: 'github-dark' };
const safeAbsoluteLinkProtocols = new Set(['https:', 'http:', 'mailto:']);

export const siteMdsvexOptions = {
  extensions: ['.svx'],
  rehypePlugins: [sanitizeLinks],
  highlight: {
    // Shiki dual-theme: each span carries both light and dark colors as
    // CSS variables; app.css picks one based on prefers-color-scheme.
    highlighter: highlightCode
  }
};

export async function highlightCode(code, lang = 'text') {
  const requestedLang = lang || 'text';
  let html;

  try {
    html = await shikiHtml(code, requestedLang);
  } catch (error) {
    if (requestedLang === 'text' || !isMissingShikiLanguage(error)) {
      throw error;
    }

    html = await shikiHtml(code, 'text');
  }

  return `{@html \`${escapeSvelte(html)}\`}`;
}

export function safeHref(value) {
  if (typeof value !== 'string') return null;

  const href = value.trim();
  if (href.length === 0 || href.startsWith('//')) return null;
  if (href.startsWith('/') || href.startsWith('#')) return href;

  try {
    const url = new URL(href);
    return safeAbsoluteLinkProtocols.has(url.protocol) ? href : null;
  } catch {
    return null;
  }
}

async function shikiHtml(code, lang) {
  return codeToHtml(code, {
    lang,
    themes: shikiThemes,
    defaultColor: false
  });
}

function isMissingShikiLanguage(error) {
  return (
    error instanceof Error &&
    /^Language `[^`]+` is not included in this bundle\./.test(error.message)
  );
}

function sanitizeLinks() {
  return (tree) => {
    visitElements(tree, (node) => {
      if (node.tagName !== 'a') return;

      const href = safeHref(node.properties?.href);
      if (href === null) {
        delete node.properties?.href;
      } else {
        node.properties.href = href;
      }
    });
  };
}

function visitElements(node, visitor) {
  if (!node || typeof node !== 'object') return;

  if (node.type === 'element') {
    visitor(node);
  }

  if (Array.isArray(node.children)) {
    for (const child of node.children) {
      visitElements(child, visitor);
    }
  }
}
