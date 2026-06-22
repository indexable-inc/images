import { mkdtemp, mkdir, writeFile, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, resolve, join, basename } from 'node:path'
import { spawnSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

function usage() {
  console.error('usage: htmlpage <page.tsx> [--out page.html] [--open]')
  process.exit(2)
}

const args = process.argv.slice(2)
const input = args.find(a => !a.startsWith('--'))
if (!input) usage()
let out = null
let open = false
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--out') out = args[++i]
  else if (args[i] === '--open') open = true
}

const inputPath = resolve(input)
const work = await mkdtemp(join(tmpdir(), 'htmlpage-'))
const moduleDir = join(work, 'node_modules', 'htmlpage')
await mkdir(join(moduleDir, 'jsx-runtime'), { recursive: true })
const runtimePath = new URL('./runtime.mjs', import.meta.url)
const runtime = await readFile(runtimePath, 'utf8')
await writeFile(join(moduleDir, 'package.json'), JSON.stringify({ type: 'module', exports: { '.': './runtime.mjs', './jsx-runtime': './jsx-runtime/index.mjs' } }))
await writeFile(join(moduleDir, 'runtime.mjs'), runtime)
await writeFile(join(moduleDir, 'jsx-runtime', 'index.mjs'), `export { Fragment, jsx, jsxs } from '../runtime.mjs'\n`)

const bundle = join(work, 'page.mjs')
const esbuild = spawnSync('esbuild', [
  inputPath,
  '--bundle',
  '--platform=node',
  '--format=esm',
  '--jsx=automatic',
  '--jsx-import-source=htmlpage',
  '--alias:htmlpage=' + join(moduleDir, 'runtime.mjs'),
  '--alias:htmlpage/jsx-runtime=' + join(moduleDir, 'jsx-runtime', 'index.mjs'),
  '--outfile=' + bundle,
], {
  cwd: work,
  encoding: 'utf8',
})
if (esbuild.status !== 0) {
  process.stderr.write(esbuild.stdout)
  process.stderr.write(esbuild.stderr)
  process.exit(esbuild.status || 1)
}

const mod = await import(pathToFileURL(bundle).href + '?t=' + Date.now())
if (typeof mod.default !== 'function') throw new Error(`${inputPath} must default-export a function`)
const html = String(await mod.default())
out = resolve(out || join(dirname(inputPath), basename(inputPath).replace(/\.[^.]+$/, '.html')))
await writeFile(out, html)
console.log(out)
if (open) spawnSync('open', [out], { stdio: 'ignore' })
