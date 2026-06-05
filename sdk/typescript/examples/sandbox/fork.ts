// Fork a running sandbox across 10 branches, each with an independent id file.
//
//   bun packages/sdk/examples/sandbox/fork.ts

import { Sandbox } from '@indexable/sdk'

await using parent = await Sandbox.ubuntu()

await parent.write('/workspace/seed.txt', 'shared by all forks\n')

const forks = await Promise.all(
	Array.from({ length: 10 }, (_, i) => parent.fork(`child-${i}`))
)

await Promise.all(
	forks.map(async (sbx, i) => {
		await sbx.write(`/workspace/id`, String(i))
		const seed = await sbx.read('/workspace/seed.txt')
		const id = await sbx.read('/workspace/id')
		console.log(`${sbx.id}: seed=${seed.trim()} id=${id}`)
		await sbx.close()
	})
)
