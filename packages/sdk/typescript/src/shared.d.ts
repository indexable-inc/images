export interface LogsOptions {
	limit: number
	since?: number
	stream?: string
}

export interface FsReadOptions {
	path: string
}

export interface FsWriteOptions {
	path: string
	text: string
	mode?: number
}

export interface FsListOptions {
	path: string
}

export interface FsReadResult {
	path: string
	text: string
}

export interface FsWriteResult {
	bytesWritten: number
}

export interface FsEntry {
	name: string
	size: number
	mode: number
	mtimeNs: number
	isDir: boolean
}

export interface SetSecretOptions {
	key: string
	value: string
}

export interface DeleteSecretOptions {
	key: string
}

export interface Secret {
	id: string
	name: string
	createdAt: number
	updatedAt: number
}

export interface RegionInfo {
	id: string
	slug: string
	displayName: string
	status: string
}

export type BranchStatus = 'Running' | 'Stopped' | 'Failed'

export interface BranchInfo {
	id: string
	name: string
	image: string
	status: BranchStatus
	ipv6: string
	ipv4?: string
	subdomain?: string
	ephemeral: boolean
	snapshotKey?: string
	forkParentVmId?: string
	forkBaseLsnNs?: string
	startedAt?: number
	stoppedAt?: number
	failureReason?: string
	createdAt: number
	updatedAt: number
	region?: RegionInfo
	ownerId: string
}
