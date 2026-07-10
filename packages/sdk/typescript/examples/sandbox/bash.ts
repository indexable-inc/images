// Bash REPL: cd / env / shell state persists across exec calls.
//
//   bun packages/sdk/typescript/examples/sandbox/bash.ts

import { Sandbox } from '@indexable/sdk'

await using sbx = await Sandbox.ubuntu()
await using sh = await sbx.repl('bash')

await sh.exec('mkdir -p /tmp/work && cd /tmp/work')
await sh.exec('export GREETING=hello')

const r = await sh.exec('pwd; echo $GREETING')
console.log('output:', r.output)
console.log('exitCode:', r.exitCode)

const fail = await sh.exec('false')
console.log('false exit code (should be 1):', fail.exitCode)

// Fire-and-forget subprocess at the sandbox level. Not part of any REPL
// session; cwd above does not carry over here.
const oneShot = await sbx.exec(['pwd'])
console.log('one-shot pwd:', oneShot.stdout.trim())
