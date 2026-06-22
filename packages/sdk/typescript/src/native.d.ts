/* Handwritten type declarations for the napi-rs native binding
 * produced by `crates/ix/sdk-ts`. Mirrors the wasm-bindgen surface
 * from `crates/ix/sdk-wasm/dist/ix_sdk.d.ts` so @indexable/sdk can
 * dispatch between the two at runtime.
 *
 * Kept in sync by hand with `crates/ix/sdk-ts/src/lib.rs` /
 * `crates/ix/sdk-ts/src/types.rs`. A Rust-side unit test in
 * `ix-sdk-ts` would be the right long-term drift guard; until
 * then, when you add a method on the Rust side, add it here too.
 */

export type BranchStatus = 'Running' | 'Stopped' | 'Failed'

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

export type ShellModeKind = 'create' | 'attach' | 'peek'

export interface ClientOptions {
	token?: string
	baseUrl?: string
}

export interface CreateOptions {
	image: string
	region: string
	name?: string
	env?: Record<string, string>
	l7ProxyPorts?: number[]
	ipv4?: boolean
}

export interface CreatePreviewOptions {
	imageTag?: string
	forkVolumes?: boolean
}

export interface ExecOptions {
	command: string[]
	workingDir?: string
}

export interface BashOptions {
	script: string
	workingDir?: string
}

export interface FsReadOptions {
	path: string
}

export interface FsWriteOptions {
	path: string
	text: string
	mode?: number
}

export interface FsReadBytesOptions {
	path: string
	offset?: number
	length?: number
}

export interface FsWriteBytesOptions {
	path: string
	data: Uint8Array
	mode?: number
}

export interface FsListOptions {
	path: string
}

export interface LogsOptions {
	limit: number
	since?: number
	stream?: string
}

export interface SetSecretOptions {
	key: string
	value: string
}

export interface DeleteSecretOptions {
	key: string
}

export interface ShellOptions {
	cols: number
	rows: number
	term?: string
	env?: Record<string, string>
	command?: string[]
	attachSession?: number
	peekSession?: number
	mode?: ShellModeKind
}

export interface SessionInfo {
	id: number
	command: string[]
	attached: boolean
	exited: boolean
}

export interface RegionInfo {
	id: string
	slug: string
	displayName: string
	status: string
}

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

export interface RuntimeStatusInfo {
	state: RuntimeState
	memoryMib?: number
	guestRpcTransportReady?: boolean
	virtioMemTransportReady?: boolean
	health?: unknown
}

export interface RuntimeStatusObservation {
	kind: 'Present' | 'Absent'
	runtime?: RuntimeStatusInfo
	absenceReason?: 'NodeAgentNotTracked' | 'VmmNotTracked'
}

export interface ExecResult {
	exitCode: number
	stdout: string
	stderr: string
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

export interface Secret {
	id: string
	name: string
	createdAt: number
	updatedAt: number
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

export interface UserInfo {
	id: string
	username: string
	displayName?: string
	email: string
	isAdmin: boolean
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

export interface ApiToken {
	id: string
	name: string
	tokenPrefix?: string
	spendLimitCents?: string
	spentCents: string
	balanceRemainingCents?: string
	createdAt: number
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

export interface CreateApiTokenResult {
	token: ApiToken
	tokenValue: string
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

export interface VmStatusEvent {
	vmId: string
	status: BranchStatus
	timestamp: number
}

export class StreamConnection {
	read(n: number): Promise<Uint8Array>
	write(data: Uint8Array): Promise<number>
	close(): Promise<void>
}

export class ShellSession {
	get sessionId(): number
	read(n: number): Promise<Uint8Array>
	write(data: Uint8Array): Promise<number>
	resize(cols: number, rows: number): Promise<void>
	close(): Promise<void>
}

export class VmStatusStream {
	next(): Promise<VmStatusEvent | null>
}

export class FsHandle {
	read(options: FsReadOptions): Promise<FsReadResult>
	write(options: FsWriteOptions): Promise<FsWriteResult>
	readBytes(options: FsReadBytesOptions): Promise<Uint8Array>
	readAllBytes(path: string): Promise<Uint8Array>
	writeAllBytes(options: FsWriteBytesOptions): Promise<number>
	list(options: FsListOptions): Promise<FsEntry[]>
}

export class SecretsHandle {
	set(options: SetSecretOptions): Promise<void>
	delete(options: DeleteSecretOptions): Promise<void>
	list(): Promise<Secret[]>
}

export class Branch {
	get id(): string
	info(): Promise<BranchInfo>
	delete(): Promise<void>
	start(): Promise<BranchInfo>
	restart(): Promise<BranchInfo>
	pause(): Promise<Commit>
	commit(): Promise<Commit>
	runtimeStatus(): Promise<RuntimeStatusObservation>
	metrics(): Promise<MetricsInfo | null>
	fork(name?: string): Promise<Branch>
	migrate(targetNodeId?: string): Promise<MigrationStart>
	cancelMigration(migrationId: string): Promise<void>
	migration(): Promise<MigrationInfo | null>
	exec(options: ExecOptions): Promise<ExecResult>
	execChecked(options: ExecOptions): Promise<ExecResult>
	bash(options: BashOptions): Promise<ExecResult>
	bashChecked(options: BashOptions): Promise<ExecResult>
	spawn(command: string[], workingDir?: string): Promise<number>
	logs(options: LogsOptions): Promise<LogEntry[]>
	log(): Promise<Commit[]>
	consoleConnect(): Promise<StreamConnection>
	portForward(port: number): Promise<StreamConnection>
	shell(options: ShellOptions): Promise<ShellSession>
	shellList(): Promise<SessionInfo[]>
	subscribeStatus(): VmStatusStream
	fs(): FsHandle
	secrets(): SecretsHandle
}

export class IxSdkClient {
	constructor(options?: ClientOptions)
	get baseUrl(): string
	regions(): Promise<RegionInfo[]>
	get(id: string): Promise<Branch>
	getByName(name: string): Promise<Branch>
	branches(): Promise<BranchInfo[]>
	buildCommitFromOci(options: CreateOptions): Promise<Commit>
	create(options: CreateOptions): Promise<Branch>
	me(): Promise<UserInfo | null>
	billingStatus(): Promise<BillingStatus>
	listApiTokens(): Promise<ApiToken[]>
	createApiToken(options: CreateApiTokenOptions): Promise<CreateApiTokenResult>
	createTopUpSession(
		options: CreateTopUpSessionOptions
	): Promise<CreateTopUpSessionResult>
	usageEvents(options: UsageEventsOptions): Promise<UsageEvent[]>
	usageSummary(options: UsageSummaryOptions): Promise<UsageSummary>
	revokeApiToken(tokenId: string): Promise<void>
	getVolume(volumeId: string): Promise<VolumeInfo>
	listVolumes(): Promise<VolumeInfo[]>
	listVolumeSnapshots(volumeId: string): Promise<VolumeSnapshot[]>
	createPreview(options: CreatePreviewOptions): Promise<PreviewDetail>
	listPreviews(): Promise<PreviewInfo[]>
	stopPreview(previewId: string): Promise<void>
	queryLogs(options: QueryLogsOptions): Promise<ObservabilityLogEntry[]>
	searchTraces(options: SearchTracesOptions): Promise<TraceSummary[]>
}
