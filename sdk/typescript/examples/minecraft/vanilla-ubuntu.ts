// Vanilla Minecraft server: Ubuntu base image + download the official jar.
//
// Installs Java, downloads the Mojang server jar, accepts the EULA,
// starts, waits for the server to be ready, and commits a snapshot.
//
// Usage:
//   bun packages/sdk/examples/minecraft/vanilla-ubuntu.ts

import { Client, type Branch, type Region } from '@indexable/sdk'

const MC_JAR_URL =
	'https://piston-data.mojang.com/v1/objects/4707d00eb834b446575d89a61a11b5d548d8c001/server.jar'

const SERVER_PROPERTIES: Record<string, string> = {
	motd: 'Ix TS SDK Minecraft',
	'enable-command-block': 'true',
	difficulty: 'normal',
	'online-mode': 'false'
}

async function waitUntilReady(
	branch: Branch,
	timeoutMs: number = 120_000,
	pollMs: number = 5_000
): Promise<void> {
	const deadline = Date.now() + timeoutMs
	const fs = branch.fs()
	while (Date.now() < deadline) {
		try {
			const { text } = await fs.read({ path: '/opt/minecraft/server.log' })
			if (text.includes('Done')) {
				return
			}
		} catch {
			// Log not present yet; keep polling until the deadline.
		}
		await new Promise((resolve) => setTimeout(resolve, pollMs))
	}
	throw new Error(`Minecraft server did not become ready within ${timeoutMs}ms`)
}

const client = new Client({
	token: process.env.IX_TOKEN ?? '',
	baseUrl: process.env.IX_API_BASE_URL ?? 'https://api.ix.dev'
})

async function pickRegion(): Promise<Region> {
	const pinned = process.env.IX_REGION
	if (pinned !== undefined && pinned !== '') {
		return pinned as Region
	}
	const regions = await client.regions()
	if (regions.length === 0) {
		throw new Error('ix API returned no regions')
	}
	return regions[0].slug as Region
}

const region = await pickRegion()

const commit = await client.buildCommitFromOci({
	image: 'ubuntu:22.04',
	region
})
const branch = await client.get(commit.branchId)

// Install Java and download server jar.
await branch.bashChecked({
	script:
		'apt-get update -qq && apt-get install -y -qq openjdk-21-jre-headless curl'
})
await branch.bashChecked({
	script: `mkdir -p /opt/minecraft && curl -sL -o /opt/minecraft/server.jar ${MC_JAR_URL}`
})

// Configure.
const fs = branch.fs()
await fs.write({ path: '/opt/minecraft/eula.txt', text: 'eula=true\n' })
const serverProperties =
	Object.entries(SERVER_PROPERTIES)
		.map(([k, v]) => `${k}=${v}`)
		.join('\n') + '\n'
await fs.write({
	path: '/opt/minecraft/server.properties',
	text: serverProperties
})

// Start, wait for ready, and commit.
await branch.spawn(
	[
		'bash',
		'-lc',
		'exec java -Xmx1024M -Xms512M -jar server.jar nogui > server.log 2>&1'
	],
	'/opt/minecraft'
)
await waitUntilReady(branch)
const snapshot = await branch.commit()
console.log(`Committed: ${snapshot.id}`)

console.log(
	'Dashboard running on ws://localhost:3300, open http://localhost:5173'
)
await new Promise((resolve) => setTimeout(resolve, 100_000))
