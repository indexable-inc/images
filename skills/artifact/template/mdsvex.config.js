import { escapeSvelte } from 'mdsvex';
import { codeToHtml } from 'shiki';

// Shiki dual-theme: every token carries both palettes as CSS variables
// (--shiki-light / --shiki-dark); app.css picks one via [data-theme].
const themes = { light: 'github-light', dark: 'github-dark' };

export const mdsvexOptions = {
  extensions: ['.svx'],
  highlight: { highlighter }
};

async function highlighter(code, lang = 'text') {
  const requested = lang || 'text';
  let html;
  try {
    html = await shikiHtml(code, requested);
  } catch (error) {
    if (requested === 'text' || !isMissingLanguage(error)) throw error;
    html = await shikiHtml(code, 'text');
  }
  return `{@html \`${escapeSvelte(html)}\`}`;
}

function shikiHtml(code, lang) {
  return codeToHtml(code, { lang, themes, defaultColor: false });
}

function isMissingLanguage(error) {
  return (
    error instanceof Error &&
    /^Language `[^`]+` is not included in this bundle\./.test(error.message)
  );
}
