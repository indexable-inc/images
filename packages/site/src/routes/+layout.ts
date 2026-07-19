export const prerender = true;
// GitHub Pages serves a directory index but has no rewrite rules, so flat
// `rfcs.html` made /rfcs work while /rfcs/ 404d. Always-trailing-slash
// prerenders every route as <path>/index.html; Pages then 301s the
// slashless form, so both spellings resolve.
export const trailingSlash = 'always';

