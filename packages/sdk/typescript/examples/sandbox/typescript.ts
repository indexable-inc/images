// TypeScript REPL via the bun harness.
//
//   bun packages/sdk/typescript/examples/sandbox/typescript.ts

import { Sandbox } from '@indexable/sdk'

await using sbx = await Sandbox.bun()
await using ts = await sbx.repl('typescript')

await ts.exec('const xs: number[] = [1, 2, 3, 4]')
const r = await ts.exec('console.log(xs.reduce((a, b) => a + b, 0))')
console.log(r.output)
