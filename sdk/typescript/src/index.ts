// Thin TS wrapper over either the wasm-bindgen bundle (browsers) or
// the napi-rs native binding (Node / Bun).
//
// Every method delegates straight to the underlying class; this file
// only adds patterns that genuinely live in JS land: `Symbol.asyncDispose`
// (cleanup on scope exit), `Symbol.asyncIterator` for the status stream,
// and buffered helpers over raw byte read/write.
//
// Runtime dispatch: if `process.versions.node` or `Bun` is present,
// load the `.node` binary via dynamic import; otherwise load the wasm
// bundle. Browsers never see the native path; Node / Bun never boot
// the WebTransport-based wasm path (they don't implement it).

import init, {
	Branch as WasmBranch,
	Client as WasmClient,
	FsHandle as WasmFsHandle,
	SecretsHandle as WasmSecretsHandle,
	ShellSession as WasmShellSession,
	StreamConnection as WasmStreamConnection,
	VmStatusStream as WasmVmStatusStream,
	// `Region` as a type + ambient `const` declaration is emitted by
	// the wasm-bindgen typescript_custom_section in
	// `crates/ix/sdk-wasm/src/region.rs`, so bringing it in here gives
	// us precise typing that stays in lockstep with Rust.
	Region as WasmRegion,
	type Region as WasmRegionType
} from '../dist/ix_sdk.js'

import type * as NativeBinding from './native.js'

// ── Runtime detection ────────────────────────────────────────────

function isNodeOrBun(): boolean {
	// Bun defines a global `Bun`; Node sets `process.versions.node`.
	// Browsers have neither. Checking `globalThis` first avoids a
	// ReferenceError on `Bun` when running under strict mode.
	const gt = globalThis as {
		Bun?: unknown
		process?: { versions?: { node?: string } }
	}
	return gt.Bun !== undefined || gt.process?.versions?.node !== undefined
}

type NativeModule = {
	IxSdkClient: new (
		options?: NativeBinding.ClientOptions
	) => NativeBinding.IxSdkClient
}

let nativeModule: NativeModule | null = null

async function loadNative(): Promise<NativeModule> {
	if (nativeModule !== null) return nativeModule
	// The binary lives alongside the wasm bundle in the published
	// package. `@indexable/sdk/native/ix_sdk.node` is the canonical
	// location; `build-native.sh` populates it. The script copies
	// the `libix_sdk_ts.so` output into that spot with the `.node`
	// suffix that napi-rs and the Node addon loader expect.
	const path = new URL('../native/ix_sdk.node', import.meta.url).pathname
	// `createRequire` is the only way Bun / Node can load an ABI
	// addon from a file path without going through `require.resolve`.
	// Dynamic `import()` of `'node:module'` via a runtime-stringified
	// specifier avoids a bundler / type-checker pulling Node types
	// into browser builds.
	const nodeModule = 'node:module'
	const mod = (await import(/* @vite-ignore */ nodeModule)) as {
		createRequire: (url: string | URL) => (id: string) => unknown
	}
	const req = mod.createRequire(import.meta.url)
	nativeModule = req(path) as NativeModule
	return nativeModule
}

// Runtime values mirror the ambient `Region` declaration emitted by
// the wasm bundle. Kept in this TS thin wrapper (not a wasm call) so
// SSR, Bun smoke tests, and module-load don't trigger wasm init.
// Keep these production constants aligned with the TypeScript custom section
// emitted by ix-region.
export const Region: typeof WasmRegion = Object.freeze({
	UsWest1: 'us-west-1',
	UsEast1: 'us-east-1'
})
export type Region = WasmRegionType

export interface ClientOptions {
	/** API token. */
	token: string
	/** API base URL. */
	baseUrl: string
}

export interface CreateOptions {
	image: string
	region: Region
	name?: string
	env?: Record<string, string>
	l7ProxyPorts?: number[]
	ipv4?: boolean
}

export type CreateOptionsForImage = Omit<CreateOptions, 'image'>

export interface ExecOptions {
	command: string[]
	workingDir?: string
}

export interface BashOptions {
	script: string
	workingDir?: string
}

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

export interface ApiTokenScope {
	resource: string
	actions: string[]
	resourceIds?: string[]
}

export interface CreateApiTokenOptions {
	name: string
	scopes: ApiTokenScope[]
	spendLimitCents?: string
	rateLimitPerMinute?: number
	expiresAt?: number
	parentToken?: string
}

export interface ApiToken {
	id: string
	name: string
	tokenPrefix?: string
	spendLimitCents?: string
	spentCents: string
	balanceRemainingCents?: string
	createdAt: number
}

export interface CreateApiTokenResult {
	token: ApiToken
	tokenValue: string
}

export interface CreatePreviewOptions {
	imageTag?: string
	forkVolumes?: boolean
}

export interface PreviewInfo {
	id: string
	imageTag: string
	status: string
	createdAt: number
	serviceCount: number
	healthyCount: number
}

export interface PreviewDetail {
	id: string
	imageTag: string
	status: string
	createdAt: number
}

export interface VolumeInfo {
	id: string
	name: string
	vmId?: string
	sizeBytes: number
	createdAt: number
	updatedAt: number
}

export interface VolumeSnapshot {
	id: string
	volumeId: string
	name: string
	manifestTimestamp: string
	createdAt: number
}

export type BillingLifecyclePhase =
	| 'active'
	| 'compute_grace'
	| 'data_retention'
	| 'deleted'

export type TopUpPaymentStatus = 'pending' | 'paid' | 'failed' | 'canceled'

export interface BillingStatus {
	balanceCents: string
	totalAddedCents: string
	spentCents: string
	lifecycle: BillingLifecycle
	topUps: TopUpPayment[]
	autoRecharge: AutoRechargeStatus
	limits: BillingLimits
}

export interface BillingLifecycle {
	phase: BillingLifecyclePhase
	zeroBalanceStartedAt?: number
	computeGraceEndsAt?: number
	dataRetentionEndsAt?: number
	deletedAt?: number
}

export interface TopUpPayment {
	id: string
	amountCents: string
	status: TopUpPaymentStatus
	savePaymentMethod: boolean
	createdAt: number
	paidAt?: number
}

export interface AutoRechargeStatus {
	enabled: boolean
	thresholdCents: string
	rechargeAmountCents: string
	paymentMethodId?: string
	paymentMethodAutoRechargeEligible: boolean
	lastFailure?: string
}

export interface BillingLimits {
	topUpPresetAmountCents: string[]
	minTopUpAmountCents: string
	maxTopUpAmountCents: string
	minAutoRechargeThresholdCents: string
	maxAutoRechargeThresholdCents: string
	minAutoRechargeAmountCents: string
	maxAutoRechargeAmountCents: string
}

export interface CreateTopUpSessionOptions {
	amountCents: string
	savePaymentMethod: boolean
}

export interface CreateTopUpSessionResult {
	url: string
	sessionId: string
	topUpId: string
	amountCents: string
	status: string
	savePaymentMethod: boolean
}

export interface UsageEventsOptions {
	tokenId: string
	since?: number
	until?: number
	resourceType?: string
	limit: number
}

export interface UsageSummaryOptions {
	since?: number
	until?: number
	resourceType?: string
	limit: number
}

export interface UsageEvent {
	id: string
	tokenId: string
	billingPoolId: string
	resourceType: string
	quantity: number
	unit: string
	costMicroUsd: string
	costCents: string
	resourceId?: string
	resourceName?: string
	createdAt: number
}

export interface UsageResourceSummary {
	resourceType: string
	totalQuantity: number
	totalCostMicroUsd: string
	totalCostCents: string
}

export interface UsageDailySummary {
	day: string
	totalCostMicroUsd: string
	totalCostCents: string
}

export interface UsageResourceInstanceSummary {
	resourceType: string
	resourceId?: string
	resourceName?: string
	totalQuantity: number
	totalCostMicroUsd: string
	totalCostCents: string
	eventCount: number
}

export interface BillingAccountActivity {
	id: string
	kind: string
	amountCents: string
	status: string
	createdAt: number
	effectiveAt?: number
}

export interface UsageSummary {
	billingPoolId: string
	since?: number
	until?: number
	totalCostMicroUsd: string
	totalCostCents: string
	resources: UsageResourceSummary[]
	recentEvents: UsageEvent[]
	dailySpend: UsageDailySummary[]
	resourceSpend: UsageResourceInstanceSummary[]
	accountActivity: BillingAccountActivity[]
}

export interface QueryLogsOptions {
	limit: number
	traceId?: string
	requestId?: string
	since?: number
	until?: number
}

export interface SearchTracesOptions {
	limit: number
	traceId?: string
	requestId?: string
	since?: number
	until?: number
}

export interface TraceSummary {
	traceId: string
	rootSpanName?: string
	serviceNames: string[]
	startedAt: number
	endedAt: number
	durationMs: number
	spanCount: number
}

export interface ObservabilityLogEntry {
	timestamp: number
	traceId?: string
	severityText?: string
	serviceName: string
	body: string
}

export type MigrationPhase =
	| 'Planning'
	| 'CapturingSource'
	| 'SourceCaptured'
	| 'PreparingTarget'
	| 'RestoringTarget'
	| 'TargetReady'
	| 'DrainingSource'
	| 'CuttingOver'
	| 'VmResumed'
	| 'Committed'
	| 'CleaningUp'
	| 'Completed'
	| 'Failed'
	| 'Cancelled'

export interface MigrationStart {
	migrationId: string
	phase: MigrationPhase
}

export interface MigrationInfo {
	migrationId: string
	vmId: string
	phase: MigrationPhase
	failureReason?: string
	createdAt: number
}

export interface MetricsInfo {
	vmId: string
	cpuPercent: number
	memoryPercent: number
	networkRxBytes: number
	networkTxBytes: number
	uptimeSecs: number
	collectedAt: number
	memoryBytes: number
	memoryLimitBytes: number
	ioReadBytes: number
	ioWriteBytes: number
}

export interface SessionInfo {
	id: number
	command: string[]
	attached: boolean
	exited: boolean
}

export interface UserInfo {
	id: string
	username: string
	displayName?: string
	email: string
	avatarUrl?: string
	createdAt: number
	usernameChangedAt?: number
	isAdmin: boolean
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

export type RuntimeState =
	| 'Unknown'
	| 'Running'
	| 'PauseRequested'
	| 'Paused'
	| 'CaptureRequested'
	| 'Captured'
	| 'ShutdownRequested'
	| 'Shutdown'
	| 'Failed'

export interface RuntimeStatusInfo {
	state: RuntimeState
	memoryMib?: number
	guestRpcTransportReady?: boolean
	virtioMemTransportReady?: boolean
	health?: RuntimeHealthInfo
}

export interface RuntimeStatusObservation {
	kind: 'Present' | 'Absent'
	runtime?: RuntimeStatusInfo
	absenceReason?: 'NodeAgentNotTracked' | 'VmmNotTracked'
}

export interface RuntimeHealthInfo {
	overall: 'Healthy' | 'Degraded' | 'Failed'
	control: { guestRpcTransport?: 'GuestRpcTransportUnavailable' }
	network: {
		vsockTransportResetReplay?: string
		vsockActivation?: string
		netActivation?: string
	}
	virtioMem: { transport?: 'TransportUnavailable' }
	capture: { request?: 'RequestFailed' }
	vcpus: Array<{
		vcpuId: number
		issue?: {
			kind: 'VcpuRun' | 'InterruptedSpin'
			errno?: number
			interruptedStreak?: number
		}
	}>
}

export interface ExecResult {
	exitCode: number
	stdout: string
	stderr: string
}

export interface LogEntry {
	timestamp: number
	vmId: string
	stream: string
	message: string
}

export interface Commit {
	id: string
	branchId: string
	parentId?: string
	status: string
	memoryMib: number
	manifestKey?: string
	createdAtMillis: number
}

export interface VmStatusEvent {
	vmId: string
	status: BranchStatus
	timestamp: number
}

export type ShellMode = 'create' | 'attach' | 'peek'

export interface ShellOptions {
	cols: number
	rows: number
	term?: string
	env?: Record<string, string>
	command?: string[]
	attachSession?: number
	peekSession?: number
	mode?: ShellMode
}

let sdkReady: Promise<void> | null = null

function ensureSdkReady(): Promise<void> {
	if (sdkReady === null) {
		const ready = init().then(() => undefined)
		sdkReady = ready
		return ready
	}
	return sdkReady
}

// ── Inner-handle type unions ─────────────────────────────────────
//
// Both the wasm-bindgen classes and the napi-rs classes expose
// identical method signatures; TypeScript accepts the union.
// Methods in the wrappers below take the union type and delegate.

type InnerClient = WasmClient | NativeBinding.IxSdkClient
type InnerBranch = WasmBranch | NativeBinding.Branch
type InnerFsHandle = WasmFsHandle | NativeBinding.FsHandle
type InnerSecretsHandle = WasmSecretsHandle | NativeBinding.SecretsHandle
type InnerShellSession = WasmShellSession | NativeBinding.ShellSession
type InnerStreamConnection =
	| WasmStreamConnection
	| NativeBinding.StreamConnection
type InnerVmStatusStream = WasmVmStatusStream | NativeBinding.VmStatusStream

// ── StreamConnection ─────────────────────────────────────────────

export class StreamConnection {
	constructor(private readonly inner: Promise<InnerStreamConnection>) {}

	async read(length: number): Promise<Uint8Array> {
		return (await this.inner).read(length)
	}

	async write(data: Uint8Array): Promise<number> {
		return (await this.inner).write(data)
	}

	async close(): Promise<void> {
		await (await this.inner).close()
	}

	async [Symbol.asyncDispose](): Promise<void> {
		await this.close()
	}
}

// ── ShellSession ─────────────────────────────────────────────────

export class ShellSession {
	constructor(private readonly inner: Promise<InnerShellSession>) {}

	get sessionId(): Promise<number> {
		return this.inner.then((s) => s.sessionId)
	}

	async read(length: number): Promise<Uint8Array> {
		return (await this.inner).read(length)
	}

	async write(data: Uint8Array): Promise<number> {
		return (await this.inner).write(data)
	}

	async resize(cols: number, rows: number): Promise<void> {
		await (await this.inner).resize(cols, rows)
	}

	async close(): Promise<void> {
		await (await this.inner).close()
	}

	async [Symbol.asyncDispose](): Promise<void> {
		await this.close()
	}
}

// ── VmStatusStream ───────────────────────────────────────────────

export class VmStatusStream {
	constructor(private readonly inner: InnerVmStatusStream) {}

	async next(): Promise<VmStatusEvent | null> {
		return (await this.inner.next()) as VmStatusEvent | null
	}

	async *[Symbol.asyncIterator](): AsyncGenerator<VmStatusEvent> {
		while (true) {
			const event = await this.next()
			if (event === null) return
			yield event
		}
	}
}

// ── FsHandle ─────────────────────────────────────────────────────

export class FsHandle {
	constructor(private readonly inner: InnerFsHandle) {}

	async read(options: FsReadOptions): Promise<FsReadResult> {
		return (await this.inner.read(options)) as FsReadResult
	}

	async write(options: FsWriteOptions): Promise<FsWriteResult> {
		return (await this.inner.write(options)) as FsWriteResult
	}

	async readBytes(
		path: string,
		offset?: number,
		length?: number
	): Promise<Uint8Array> {
		return this.inner.readBytes(path, offset, length)
	}

	async readAllBytes(path: string): Promise<Uint8Array> {
		return this.inner.readAllBytes(path)
	}

	async writeAllBytes(
		path: string,
		data: Uint8Array,
		mode?: number
	): Promise<number> {
		return this.inner.writeAllBytes(path, data, mode)
	}

	async list(options: FsListOptions): Promise<FsEntry[]> {
		return (await this.inner.list(options)) as FsEntry[]
	}
}

// ── SecretsHandle ────────────────────────────────────────────────

export class SecretsHandle {
	constructor(private readonly inner: InnerSecretsHandle) {}

	async set(options: SetSecretOptions): Promise<void> {
		await this.inner.set(options)
	}

	async delete(options: DeleteSecretOptions): Promise<void> {
		await this.inner.delete(options)
	}

	async list(): Promise<Secret[]> {
		return (await this.inner.list()) as Secret[]
	}
}

// ── Branch ───────────────────────────────────────────────────────

export class Branch {
	constructor(private readonly inner: InnerBranch) {}

	get id(): string {
		return this.inner.id
	}

	fs(): FsHandle {
		return new FsHandle(this.inner.fs())
	}

	secrets(): SecretsHandle {
		return new SecretsHandle(this.inner.secrets())
	}

	async info(): Promise<BranchInfo> {
		return (await this.inner.info()) as BranchInfo
	}

	async delete(): Promise<void> {
		await this.inner.delete()
	}

	async [Symbol.asyncDispose](): Promise<void> {
		await this.delete()
	}

	async start(): Promise<BranchInfo> {
		return (await this.inner.start()) as BranchInfo
	}

	async restart(): Promise<BranchInfo> {
		return (await this.inner.restart()) as BranchInfo
	}

	async pause(): Promise<Commit> {
		return (await this.inner.pause()) as Commit
	}

	async commit(): Promise<Commit> {
		return (await this.inner.commit()) as Commit
	}

	async runtimeStatus(): Promise<RuntimeStatusInfo | null> {
		const observation =
			(await this.inner.runtimeStatus()) as RuntimeStatusObservation
		return observation.runtime ?? null
	}

	async runtimeStatusObservation(): Promise<RuntimeStatusObservation> {
		return (await this.inner.runtimeStatus()) as RuntimeStatusObservation
	}

	async metrics(): Promise<MetricsInfo | null> {
		return (await this.inner.metrics()) as MetricsInfo | null
	}

	async fork(name?: string): Promise<Branch> {
		return new Branch(await this.inner.fork(name))
	}

	async migrate(targetNodeId?: string): Promise<MigrationStart> {
		return (await this.inner.migrate(targetNodeId)) as MigrationStart
	}

	async cancelMigration(migrationId: string): Promise<void> {
		await this.inner.cancelMigration(migrationId)
	}

	async migration(): Promise<MigrationInfo | null> {
		return (await this.inner.migration()) as MigrationInfo | null
	}

	async exec(options: ExecOptions): Promise<ExecResult> {
		return (await this.inner.exec(options)) as ExecResult
	}

	async execChecked(options: ExecOptions): Promise<ExecResult> {
		return (await this.inner.execChecked(options)) as ExecResult
	}

	async bash(options: BashOptions): Promise<ExecResult> {
		return (await this.inner.bash(options)) as ExecResult
	}

	async bashChecked(options: BashOptions): Promise<ExecResult> {
		return (await this.inner.bashChecked(options)) as ExecResult
	}

	async spawn(command: string[], workingDir?: string): Promise<number> {
		return this.inner.spawn(command, workingDir)
	}

	async logs(options: LogsOptions): Promise<LogEntry[]> {
		return (await this.inner.logs(options)) as LogEntry[]
	}

	async log(): Promise<Commit[]> {
		return (await this.inner.log()) as Commit[]
	}

	async consoleConnect(): Promise<StreamConnection> {
		return new StreamConnection(this.inner.consoleConnect())
	}

	async portForward(port: number): Promise<StreamConnection> {
		return new StreamConnection(this.inner.portForward(port))
	}

	async shell(options: ShellOptions): Promise<ShellSession> {
		return new ShellSession(this.inner.shell(options))
	}

	async shellList(): Promise<SessionInfo[]> {
		return (await this.inner.shellList()) as SessionInfo[]
	}

	subscribeStatus(): VmStatusStream {
		return new VmStatusStream(this.inner.subscribeStatus())
	}
}

// ── Client ───────────────────────────────────────────────────────

export class Client {
	private readonly inner: Promise<InnerClient>

	constructor(options: ClientOptions) {
		if (isNodeOrBun()) {
			// Node / Bun: load the `.node` binary. No WebTransport in
			// these runtimes, so the wasm bundle can't reach the API.
			this.inner = loadNative().then((m) => new m.IxSdkClient(options))
		} else {
			// Browser: wasm-bindgen bundle over native WebTransport.
			this.inner = ensureSdkReady().then(() => new WasmClient(options))
		}
	}

	get baseUrl(): Promise<string> {
		return this.inner.then((c) => c.baseUrl)
	}

	async regions(): Promise<RegionInfo[]> {
		return (await (await this.inner).regions()) as RegionInfo[]
	}

	async get(id: string): Promise<Branch> {
		return new Branch(await (await this.inner).get(id))
	}

	async getByName(name: string): Promise<Branch> {
		return new Branch(await (await this.inner).getByName(name))
	}

	async branches(): Promise<BranchInfo[]> {
		return (await (await this.inner).branches()) as BranchInfo[]
	}

	async currentUsername(): Promise<string> {
		const user = await this.me()
		if (user === null) throw new Error('ix API returned no authenticated user')
		return user.username
	}

	async create(
		image: string,
		options: CreateOptionsForImage,
		onProgress?: (label: string) => void
	): Promise<Branch>
	async create(
		options: CreateOptions,
		onProgress?: (label: string) => void
	): Promise<Branch>
	async create(
		imageOrOptions: string | CreateOptions,
		optionsOrProgress?: CreateOptionsForImage | ((label: string) => void),
		maybeProgress?: (label: string) => void
	): Promise<Branch> {
		const options =
			typeof imageOrOptions === 'string'
				? {
						...(optionsOrProgress as CreateOptionsForImage),
						image: imageOrOptions
					}
				: imageOrOptions
		const onProgress =
			typeof imageOrOptions === 'string'
				? maybeProgress
				: (optionsOrProgress as ((label: string) => void) | undefined)
		const inner = await this.inner
		// wasm variant takes a second `onProgress` callback; napi one
		// doesn't yet. Pass through when supported, drop otherwise.
		// TODO(sdk-ts): plumb progress events through napi-rs by
		// mirroring wasm-bindgen's `Option<js_sys::Function>`.
		const out =
			'length' in inner.create && inner.create.length >= 2
				? await (
						inner.create as (
							o: CreateOptions,
							cb?: (label: string) => void
						) => Promise<InnerBranch>
					)(options, onProgress)
				: await (inner.create as (o: CreateOptions) => Promise<InnerBranch>)(
						options
					)
		return new Branch(out)
	}

	async buildCommitFromOci(options: CreateOptions): Promise<Commit> {
		return (await (await this.inner).buildCommitFromOci(options)) as Commit
	}

	async me(): Promise<UserInfo | null> {
		return (await (await this.inner).me()) as UserInfo | null
	}

	async billingStatus(): Promise<BillingStatus> {
		return (await (await this.inner).billingStatus()) as BillingStatus
	}

	async listApiTokens(): Promise<ApiToken[]> {
		return (await (await this.inner).listApiTokens()) as ApiToken[]
	}

	async createApiToken(
		options: CreateApiTokenOptions
	): Promise<CreateApiTokenResult> {
		return (await (
			await this.inner
		).createApiToken(options)) as CreateApiTokenResult
	}

	async createTopUpSession(
		options: CreateTopUpSessionOptions
	): Promise<CreateTopUpSessionResult> {
		return (await (
			await this.inner
		).createTopUpSession(options)) as CreateTopUpSessionResult
	}

	async usageEvents(options: UsageEventsOptions): Promise<UsageEvent[]> {
		return (await (await this.inner).usageEvents(options)) as UsageEvent[]
	}

	async usageSummary(options: UsageSummaryOptions): Promise<UsageSummary> {
		return (await (await this.inner).usageSummary(options)) as UsageSummary
	}

	async revokeApiToken(tokenId: string): Promise<void> {
		await (await this.inner).revokeApiToken(tokenId)
	}

	async getVolume(volumeId: string): Promise<VolumeInfo> {
		return (await (await this.inner).getVolume(volumeId)) as VolumeInfo
	}

	async listVolumes(): Promise<VolumeInfo[]> {
		return (await (await this.inner).listVolumes()) as VolumeInfo[]
	}

	async listVolumeSnapshots(volumeId: string): Promise<VolumeSnapshot[]> {
		return (await (
			await this.inner
		).listVolumeSnapshots(volumeId)) as VolumeSnapshot[]
	}

	async createPreview(
		options: CreatePreviewOptions = {}
	): Promise<PreviewDetail> {
		return (await (await this.inner).createPreview(options)) as PreviewDetail
	}

	async listPreviews(): Promise<PreviewInfo[]> {
		return (await (await this.inner).listPreviews()) as PreviewInfo[]
	}

	async stopPreview(previewId: string): Promise<void> {
		await (await this.inner).stopPreview(previewId)
	}

	async queryLogs(options: QueryLogsOptions): Promise<ObservabilityLogEntry[]> {
		return (await (
			await this.inner
		).queryLogs(options)) as ObservabilityLogEntry[]
	}

	async searchTraces(options: SearchTracesOptions): Promise<TraceSummary[]> {
		return (await (await this.inner).searchTraces(options)) as TraceSummary[]
	}
}

// ── Sandbox ──────────────────────────────────────────────────────
//
// Opinionated surface over Client + Branch. The shape is:
//
//   await using sbx = await Sandbox.oci('python:3.12')
//   await sbx.exec(['ls'])                 // fire-and-forget subprocess
//   await using py = await sbx.repl('python')
//   await py.exec('x = 42')                // stateful: persists for next exec
//   await py.exec('print(x)')              // sees x
//
// A Repl is a long-lived interpreter process. State (variables, cwd, env)
// persists across `exec` calls. Independent sessions are independent: open
// two repls and they don't share state. `sbx.exec` is the non-REPL escape
// hatch for "just run a binary and give me the result."

function readEnv(): Record<string, string | undefined> {
	const gt = globalThis as {
		process?: { env?: Record<string, string | undefined> }
	}
	return gt.process?.env ?? {}
}

export interface SandboxOptions {
	/** API token. Falls back to IX_TOKEN then IX_API_KEY in the environment. */
	token?: string
	/** API base URL. Falls back to IX_API_BASE_URL then https://api.ix.dev. */
	baseUrl?: string
	/** Region. Falls back to IX_REGION then the first region the API returns. */
	region?: Region
	/** Environment variables set inside the VM. */
	env?: Record<string, string>
	/** Human-readable name for the VM. */
	name?: string
	/** Allocate a public IPv4. */
	ipv4?: boolean
	/** Ports to expose through the L7 proxy. */
	l7ProxyPorts?: number[]
}

export type ReplLanguage = 'bash' | 'python' | 'node' | 'bun' | 'typescript'

export interface ReplOptions {
	cols?: number
	rows?: number
}

export interface ReplResult {
	/** Combined stdout+stderr as rendered through the PTY. */
	output: string
	/** Exit status reported by the wrapper. `0` on success, non-zero on error. */
	exitCode: number
}

const REPL_END = '__IX_REPL_END__'

// Python harness: reads `EXEC:<marker>:<base64>` lines from stdin, execs
// the decoded code under a persistent namespace, then prints a sentinel
// line that the client scans for. Stays simple on purpose; `exec` keeps
// the existing `ns` dict so `x = 1` then `print(x)` works across calls.
const PYTHON_HARNESS = `
import sys, base64, traceback
ns = {'__name__': '__main__', '__builtins__': __builtins__}
while True:
    line = sys.stdin.readline()
    if not line:
        break
    line = line.rstrip()
    if not line:
        continue
    try:
        kind, marker, payload = line.split(':', 2)
    except ValueError:
        continue
    if kind != 'EXEC':
        continue
    try:
        src = base64.b64decode(payload).decode('utf-8')
        exec(compile(src, '<repl>', 'exec'), ns)
        print(f'${REPL_END}:{marker}:0', flush=True)
    except SystemExit as _e:
        rc = _e.code if isinstance(_e.code, int) else 1
        print(f'${REPL_END}:{marker}:{rc}', flush=True)
    except BaseException:
        traceback.print_exc()
        sys.stdout.flush()
        print(f'${REPL_END}:{marker}:1', flush=True)
`.trim()

// Node / Bun harness: same wire format as PYTHON_HARNESS, but on top of
// a shared `vm.createContext` so redeclarations (`let x`) persist
// between calls the way a REPL user expects.
const JS_HARNESS = `
const readline = require('node:readline')
const vm = require('node:vm')
const ctx = vm.createContext({ console, require, Buffer, process, globalThis })
const rl = readline.createInterface({ input: process.stdin })
rl.on('line', (line) => {
  const i1 = line.indexOf(':')
  if (i1 < 0) return
  const kind = line.slice(0, i1)
  const rest = line.slice(i1 + 1)
  const i2 = rest.indexOf(':')
  if (i2 < 0) return
  const marker = rest.slice(0, i2)
  const payload = rest.slice(i2 + 1)
  if (kind !== 'EXEC') return
  try {
    const src = Buffer.from(payload, 'base64').toString('utf-8')
    vm.runInContext(src, ctx, { displayErrors: true })
    console.log('${REPL_END}:' + marker + ':0')
  } catch (e) {
    console.error(e && e.stack ? e.stack : String(e))
    console.log('${REPL_END}:' + marker + ':1')
  }
})
`.trim()

function replCommand(lang: ReplLanguage): string[] {
	switch (lang) {
		case 'bash':
			return ['bash', '--norc', '--noprofile']
		case 'python':
			return ['python3', '-u', '-c', PYTHON_HARNESS]
		case 'node':
			return ['node', '-e', JS_HARNESS]
		case 'bun':
		case 'typescript':
			return ['bun', '-e', JS_HARNESS]
	}
}

function b64encodeUtf8(s: string): string {
	const bytes = new TextEncoder().encode(s)
	let bin = ''
	for (const byte of bytes) bin += String.fromCharCode(byte)
	return btoa(bin)
}

function randomMarker(): string {
	const gt = globalThis as { crypto?: { randomUUID?: () => string } }
	if (gt.crypto?.randomUUID !== undefined)
		return gt.crypto.randomUUID().replace(/-/g, '')
	let s = ''
	for (let i = 0; i < 32; i++) s += Math.floor(Math.random() * 16).toString(16)
	return s
}

export class Repl {
	private readonly decoder = new TextDecoder()
	private buffer = ''
	private closed = false

	private constructor(
		private readonly session: ShellSession,
		private readonly language: ReplLanguage
	) {}

	static async open(
		branch: Branch,
		language: ReplLanguage,
		options: ReplOptions = {}
	): Promise<Repl> {
		const session = await branch.shell({
			cols: options.cols ?? 200,
			rows: options.rows ?? 50,
			term: 'dumb',
			command: replCommand(language),
			mode: 'create'
		})
		const repl = new Repl(session, language)
		if (language === 'bash') {
			// stty off so the PTY doesn't echo our input back into the output
			// stream; PS1/PS2 empty so prompts don't appear between results.
			await repl.exec(
				`stty -echo -onlcr 2>/dev/null; export PS1=''; export PS2=''; set +o history`
			)
		}
		return repl
	}

	get lang(): ReplLanguage {
		return this.language
	}

	async exec(code: string): Promise<ReplResult> {
		if (this.closed) throw new Error('Repl is closed')
		const marker = randomMarker()
		await this.write(this.frame(code, marker))
		const endPrefix = `${REPL_END}:${marker}:`
		const raw = await this.readUntil(endPrefix)
		return this.splitAtSentinel(raw, endPrefix)
	}

	async close(): Promise<void> {
		if (this.closed) return
		this.closed = true
		await this.session.close()
	}

	async [Symbol.asyncDispose](): Promise<void> {
		await this.close()
	}

	private frame(code: string, marker: string): Uint8Array {
		let line: string
		if (this.language === 'bash') {
			line = `{ ${code}\n}; __IX_RC=$?; printf '\\n${REPL_END}:${marker}:%d\\n' $__IX_RC\n`
		} else {
			line = `EXEC:${marker}:${b64encodeUtf8(code)}\n`
		}
		return new TextEncoder().encode(line)
	}

	private splitAtSentinel(raw: string, endPrefix: string): ReplResult {
		const idx = raw.lastIndexOf(endPrefix)
		const before = raw.slice(0, idx).replace(/\r\n/g, '\n').replace(/\r/g, '')
		const tail = raw.slice(idx + endPrefix.length)
		const match = tail.match(/^(-?\d+)/)
		const exitCode = match === null ? -1 : Number.parseInt(match[1], 10)
		return { output: before.replace(/\n$/, ''), exitCode }
	}

	private async write(data: Uint8Array): Promise<void> {
		let sent = 0
		while (sent < data.length) {
			const n = await this.session.write(data.subarray(sent))
			if (n <= 0) throw new Error('Repl stdin write returned 0')
			sent += n
		}
	}

	private async readUntil(needle: string): Promise<string> {
		while (true) {
			const hit = this.buffer.indexOf(needle)
			if (hit !== -1) {
				const eol = this.buffer.indexOf('\n', hit)
				const cut = eol === -1 ? this.buffer.length : eol + 1
				const chunk = this.buffer.slice(0, cut)
				this.buffer = this.buffer.slice(cut)
				return chunk
			}
			const bytes = await this.session.read(4096)
			if (bytes.length === 0) throw new Error('Repl EOF before sentinel')
			this.buffer += this.decoder.decode(bytes, { stream: true })
		}
	}
}

export class Sandbox {
	private constructor(
		private readonly _client: Client,
		private readonly _branch: Branch
	) {}

	/** Launch from any OCI image. `image` is a tag like `python:3.12` or a full registry ref. */
	static async oci(
		image: string,
		options: SandboxOptions = {}
	): Promise<Sandbox> {
		const client = Sandbox.buildClient(options)
		const region = await Sandbox.resolveRegion(client, options.region)
		const branch = await client.create(image, {
			region,
			env: options.env,
			name: options.name,
			ipv4: options.ipv4,
			l7ProxyPorts: options.l7ProxyPorts
		})
		return new Sandbox(client, branch)
	}

	/** Ubuntu convenience. Same as `Sandbox.oci('ubuntu:<version>')`. */
	static ubuntu(
		version = '24.04',
		options: SandboxOptions = {}
	): Promise<Sandbox> {
		return Sandbox.oci(`ubuntu:${version}`, options)
	}

	/** Python convenience. Same as `Sandbox.oci('python:<version>')`. */
	static python(
		version = '3.12',
		options: SandboxOptions = {}
	): Promise<Sandbox> {
		return Sandbox.oci(`python:${version}`, options)
	}

	/** Node.js convenience. Same as `Sandbox.oci('node:<version>')`. */
	static node(version = '22', options: SandboxOptions = {}): Promise<Sandbox> {
		return Sandbox.oci(`node:${version}`, options)
	}

	/** Bun convenience. Same as `Sandbox.oci('oven/bun:<version>')`. */
	static bun(
		version = 'latest',
		options: SandboxOptions = {}
	): Promise<Sandbox> {
		return Sandbox.oci(`oven/bun:${version}`, options)
	}

	/** Adopt an existing VM. `token` / `baseUrl` resolve from options then env. */
	static async attach(
		vmId: string,
		options: Pick<SandboxOptions, 'token' | 'baseUrl'> = {}
	): Promise<Sandbox> {
		const client = Sandbox.buildClient(options)
		const branch = await client.get(vmId)
		return new Sandbox(client, branch)
	}

	get id(): string {
		return this._branch.id
	}

	get branch(): Branch {
		return this._branch
	}

	get client(): Client {
		return this._client
	}

	info(): Promise<BranchInfo> {
		return this._branch.info()
	}

	/** Fire-and-forget subprocess. No state persists between calls; each `exec` is a fresh process. */
	exec(command: string[], opts: { cwd?: string } = {}): Promise<ExecResult> {
		return this._branch.exec({ command, workingDir: opts.cwd })
	}

	/**
	 * Open a stateful REPL. State (variables, cwd, env) persists across `exec`
	 * calls on the returned handle. Open multiple repls for independent sessions.
	 *
	 *   await using py = await sbx.repl('python')
	 *   await py.exec('x = 42')
	 *   await py.exec('print(x)')     // sees x
	 */
	repl(language: ReplLanguage, options: ReplOptions = {}): Promise<Repl> {
		return Repl.open(this._branch, language, options)
	}

	async read(path: string): Promise<string> {
		const r = await this._branch.fs().read({ path })
		return r.text
	}

	async write(path: string, text: string, mode?: number): Promise<number> {
		const r = await this._branch.fs().write({ path, text, mode })
		return r.bytesWritten
	}

	readBytes(
		path: string,
		offset?: number,
		length?: number
	): Promise<Uint8Array> {
		return this._branch.fs().readBytes(path, offset, length)
	}

	writeBytes(path: string, data: Uint8Array, mode?: number): Promise<number> {
		return this._branch.fs().writeAllBytes(path, data, mode)
	}

	list(path: string): Promise<FsEntry[]> {
		return this._branch.fs().list({ path })
	}

	async fork(name?: string): Promise<Sandbox> {
		const b = await this._branch.fork(name)
		return new Sandbox(this._client, b)
	}

	close(): Promise<void> {
		return this._branch.delete()
	}

	async [Symbol.asyncDispose](): Promise<void> {
		await this.close()
	}

	private static buildClient(
		options: Pick<SandboxOptions, 'token' | 'baseUrl'>
	): Client {
		const env = readEnv()
		const token = options.token ?? env.IX_TOKEN ?? env.IX_API_KEY
		if (token === undefined || token === '') {
			throw new Error(
				'Sandbox: no API token. Pass { token } or set IX_TOKEN / IX_API_KEY.'
			)
		}
		const baseUrl =
			options.baseUrl ?? env.IX_API_BASE_URL ?? 'https://api.ix.dev'
		return new Client({ token, baseUrl })
	}

	private static async resolveRegion(
		client: Client,
		pinned?: Region
	): Promise<Region> {
		if (pinned !== undefined) return pinned
		const fromEnv = readEnv().IX_REGION
		if (fromEnv !== undefined && fromEnv !== '') return fromEnv as Region
		const regions = await client.regions()
		if (regions.length === 0) throw new Error('ix API returned no regions')
		return regions[0].slug as Region
	}
}
