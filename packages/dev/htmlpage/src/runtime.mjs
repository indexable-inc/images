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
  github: '<path d="M12 .5a12 12 0 0 0-3.79 23.39c.6.11.82-.26.82-.58v-2.03c-3.34.73-4.04-1.61-4.04-1.61-.55-1.39-1.34-1.76-1.34-1.76-1.09-.75.08-.73.08-.73 1.2.08 1.84 1.24 1.84 1.24 1.07 1.83 2.81 1.3 3.5.99.11-.78.42-1.3.76-1.6-2.67-.3-5.47-1.33-5.47-5.93 0-1.31.47-2.38 1.24-3.22-.12-.3-.54-1.52.12-3.18 0 0 1.01-.32 3.3 1.23A11.5 11.5 0 0 1 12 5.8c1.02 0 2.05.14 3.01.4 2.29-1.55 3.3-1.23 3.3-1.23.66 1.66.24 2.88.12 3.18.77.84 1.24 1.91 1.24 3.22 0 4.61-2.81 5.63-5.48 5.93.43.37.81 1.1.81 2.22v3.29c0 .32.22.7.83.58A12 12 0 0 0 12 .5Z"/>',
  pr: '<path d="M6 3a3 3 0 1 0 1 5.83V15a5 5 0 0 0 5 5h3.17a3 3 0 1 0 0-2H12a3 3 0 0 1-3-3V8.83A3 3 0 0 0 6 3Zm0 2a1 1 0 1 1 0 2 1 1 0 0 1 0-2Zm12 12a1 1 0 1 1 0 2 1 1 0 0 1 0-2Z"/>',
  check: '<path d="M9.55 17.6 4.3 12.35l1.4-1.4 3.85 3.85 8.75-8.75 1.4 1.4-10.15 10.15Z"/>',
  issue: '<path d="M12 2a10 10 0 1 0 .01 0ZM11 6h2v8h-2V6Zm0 10h2v2h-2v-2Z"/>',
  commit: '<path d="M17 11a5 5 0 0 0-9.9 0H2v2h5.1a5 5 0 0 0 9.8 0H22v-2h-5Zm-5 5a3 3 0 1 1 0-6 3 3 0 0 1 0 6Z"/>',
  link: '<path d="M10.6 13.4a1 1 0 0 1 0-1.4l3.4-3.4a3 3 0 0 1 4.2 4.2l-2 2a1 1 0 1 1-1.4-1.4l2-2a1 1 0 0 0-1.4-1.4L12 13.4a1 1 0 0 1-1.4 0Zm2.8-2.8a1 1 0 0 1 0 1.4L10 15.4a3 3 0 0 1-4.2-4.2l2-2a1 1 0 1 1 1.4 1.4l-2 2A1 1 0 1 0 8.6 14l3.4-3.4a1 1 0 0 1 1.4 0Z"/>'
}

export function Icon({ name = 'check', label = '', className = 'icon' }) {
  return `<svg class="${escapeAttr(className)}" viewBox="0 0 24 24" role="img" aria-label="${escapeAttr(label || name)}" fill="currentColor">${iconPaths[name] || iconPaths.check}</svg>`
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
