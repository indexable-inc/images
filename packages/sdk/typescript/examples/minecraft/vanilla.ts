// Vanilla Minecraft server: using the itzg/minecraft-server public image.
//
// The simplest approach. The image handles Java, the server jar, EULA
// acceptance, and startup automatically via environment variables.
//
// Usage:
//   bun packages/sdk/typescript/examples/minecraft/vanilla.ts

import { Client, type Region } from '@indexable/sdk'

const MINECRAFT_ENV: Record<string, string> = {
	EULA: 'TRUE',
	TYPE: 'VANILLA',
	ONLINE_MODE: 'FALSE',
	MEMORY: '4G',
	MOTD: 'Ix TS SDK Vanilla Minecraft',
	PAUSE_WHEN_EMPTY_SECONDS: '-1'
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

const branch = await client.create({
	image: 'itzg/minecraft-server:latest',
	region: region,
	env: MINECRAFT_ENV,
	ipv4: true
})

console.log(`Created branch: ${branch.id}`)
