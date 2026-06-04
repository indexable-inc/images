// Python REPL: state persists across exec calls on the same Repl.
//
//   bun packages/sdk/examples/sandbox/python.ts

import { Sandbox } from '@indexable/sdk'

await using sbx = await Sandbox.python()
await using py = await sbx.repl('python')

await py.exec('import math')
const a = await py.exec('x = 42')
console.log('a:', a)

const b = await py.exec('print(x * 2, math.pi)')
console.log('b.output:', b.output)
console.log('b.exitCode:', b.exitCode)

// An independent session does not see `x`.
await using py2 = await sbx.repl('python')
const c = await py2.exec('print(x)')
console.log('c.exitCode (should be 1):', c.exitCode)
console.log('c.output:', c.output)
