const voidTags = new Set(['area','base','br','col','embed','hr','img','input','link','meta','param','source','track','wbr'])

export const Fragment = Symbol.for('htmlpage.fragment')

function escapeText(value) {
  return String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')
}

function escapeAttr(value) {
  return escapeText(value).replaceAll('"', '&quot;')
}

function kebab(name) {
  return name.replace(/[A-Z]/g, c => `-${c.toLowerCase()}`)
}

function styleObject(style) {
  if (!style || typeof style !== 'object') return style
  return Object.entries(style).map(([k, v]) => `${kebab(k)}:${v}`).join(';')
}

function flat(value, out = []) {
  if (Array.isArray(value)) for (const item of value) flat(item, out)
  else if (value !== false && value !== true && value != null) out.push(value)
  return out
}

function attrs(props = {}) {
  let out = ''
  for (const [rawName, rawValue] of Object.entries(props)) {
    if (rawName === 'children' || rawValue === false || rawValue == null) continue
    const name = rawName === 'className' ? 'class' : rawName === 'htmlFor' ? 'for' : kebab(rawName)
    const value = rawName === 'style' ? styleObject(rawValue) : rawValue
    out += value === true ? ` ${name}` : ` ${name}="${escapeAttr(value)}"`
  }
  return out
}

export function render(value) {
  return flat(value).map(v => typeof v === 'string' ? v : escapeText(v)).join('')
}

export function jsx(type, props = {}) {
  if (type === Fragment) return render(props.children)
  if (typeof type === 'function') return type(props)
  const body = render(props.children)
  return voidTags.has(type) ? `<${type}${attrs(props)}>` : `<${type}${attrs(props)}>${body}</${type}>`
}
export const jsxs = jsx

const iconPaths = {
  // Primer Octicons v19 16px paths.
  markGithub: '<path d="M6.766 11.328c-2.063-.25-3.516-1.734-3.516-3.656 0-.781.281-1.625.75-2.188-.203-.515-.172-1.609.063-2.062.625-.078 1.468.25 1.968.703.594-.187 1.219-.281 1.985-.281.765 0 1.39.094 1.953.265.484-.437 1.344-.765 1.969-.687.218.422.25 1.515.046 2.047.5.593.766 1.39.766 2.203 0 1.922-1.453 3.375-3.547 3.64.531.344.89 1.094.89 1.954v1.625c0 .468.391.734.86.547C13.781 14.359 16 11.53 16 8.03 16 3.61 12.406 0 7.984 0 3.563 0 0 3.61 0 8.031a7.88 7.88 0 0 0 5.172 7.422c.422.156.828-.125.828-.547v-1.25c-.219.094-.5.156-.75.156-1.031 0-1.64-.562-2.078-1.609-.172-.422-.36-.672-.719-.719-.187-.015-.25-.093-.25-.187 0-.188.313-.328.625-.328.453 0 .844.281 1.25.86.313.452.64.655 1.031.655s.641-.14 1-.5c.266-.265.47-.5.657-.656"/>',
  gitPullRequest: '<path d="M1.5 3.25a2.25 2.25 0 1 1 3 2.122v5.256a2.251 2.251 0 1 1-1.5 0V5.372A2.25 2.25 0 0 1 1.5 3.25Zm5.677-.177L9.573.677A.25.25 0 0 1 10 .854V2.5h1A2.5 2.5 0 0 1 13.5 5v5.628a2.251 2.251 0 1 1-1.5 0V5a1 1 0 0 0-1-1h-1v1.646a.25.25 0 0 1-.427.177L7.177 3.427a.25.25 0 0 1 0-.354ZM3.75 2.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm0 9.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm8.25.75a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Z"/>',
  checkCircle: '<path d="M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm1.5 0a6.5 6.5 0 1 0 13 0 6.5 6.5 0 0 0-13 0Zm10.28-1.72-4.5 4.5a.75.75 0 0 1-1.06 0l-2-2a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018l1.47 1.47 3.97-3.97a.751.751 0 0 1 1.042.018.751.751 0 0 1 .018 1.042Z"/>',
  issueOpened: '<path d="M8 9.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z"/><path d="M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0ZM1.5 8a6.5 6.5 0 1 0 13 0 6.5 6.5 0 0 0-13 0Z"/>',
  gitCommit: '<path d="M11.93 8.5a4.002 4.002 0 0 1-7.86 0H.75a.75.75 0 0 1 0-1.5h3.32a4.002 4.002 0 0 1 7.86 0h3.32a.75.75 0 0 1 0 1.5Zm-1.43-.75a2.5 2.5 0 1 0-5 0 2.5 2.5 0 0 0 5 0Z"/>',
  link: '<path d="m7.775 3.275 1.25-1.25a3.5 3.5 0 1 1 4.95 4.95l-2.5 2.5a3.5 3.5 0 0 1-4.95 0 .751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018 1.998 1.998 0 0 0 2.83 0l2.5-2.5a2.002 2.002 0 0 0-2.83-2.83l-1.25 1.25a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042Zm-4.69 9.64a1.998 1.998 0 0 0 2.83 0l1.25-1.25a.751.751 0 0 1 1.042.018.751.751 0 0 1 .018 1.042l-1.25 1.25a3.5 3.5 0 1 1-4.95-4.95l2.5-2.5a3.5 3.5 0 0 1 4.95 0 .751.751 0 0 1-.018 1.042.751.751 0 0 1-1.042.018 1.998 1.998 0 0 0-2.83 0l-2.5 2.5a1.998 1.998 0 0 0 0 2.83Z"/>'
}

iconPaths.github = iconPaths.markGithub
iconPaths.pr = iconPaths.gitPullRequest
iconPaths.check = iconPaths.checkCircle
iconPaths.issue = iconPaths.issueOpened
iconPaths.commit = iconPaths.gitCommit

export function Icon({ name = 'check', label = '', className = 'icon' }) {
  return `<svg class="${escapeAttr(className)}" viewBox="0 0 16 16" role="img" aria-label="${escapeAttr(label || name)}" fill="currentColor">${iconPaths[name] || iconPaths.check}</svg>`
}

export const icons = Object.fromEntries(Object.keys(iconPaths).map(name => [name, props => Icon({ name, ...props })]))

export function Page({ title = 'Report', children }) {
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>${escapeText(title)}</title><style>${defaultCss}</style></head><body><main>${render(children)}</main></body></html>`
}

export function Card({ title, icon, children, href }) {
  const head = title ? `<h2>${icon ? render(icon) + ' ' : ''}${escapeText(title)}</h2>` : ''
  const body = `<section class="card">${head}${render(children)}</section>`
  return href ? `<a class="card-link" href="${escapeAttr(href)}">${body}</a>` : body
}

export function Link({ href, icon, children }) {
  return `<a href="${escapeAttr(href)}">${icon ? render(icon) + ' ' : ''}${render(children)}</a>`
}

export function Code({ children }) {
  return `<code>${escapeText(render(children))}</code>`
}

export const defaultCss = `
body{font-family:system-ui,-apple-system,sans-serif;margin:0;background:#f7f9fc;color:#172033;line-height:1.45}
main{max-width:900px;margin:2rem auto;padding:0 1rem}.hero,.card{background:white;border:1px solid #dfe6f3;border-radius:18px;padding:1.2rem;margin:1rem 0;box-shadow:0 12px 32px #1720330d}
h1{font-size:1.55rem;margin:.1rem 0 .4rem}h2{font-size:1.1rem;margin:.2rem 0 .6rem}a{color:#0969da;font-weight:700;text-decoration:none}a:hover{text-decoration:underline}
code,pre{background:#f1f4f9;border:1px solid #dfe6f3;border-radius:8px}code{padding:.08rem .3rem}pre{padding:1rem;overflow:auto}.icon{width:1.15em;height:1.15em;vertical-align:-.18em;display:inline-block}li{margin:.35rem 0}.card-link{display:block;color:inherit}.card-link:hover{text-decoration:none;transform:translateY(-1px)}
`
