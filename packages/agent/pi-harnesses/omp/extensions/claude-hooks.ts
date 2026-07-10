/**
 * claude-hooks: Claude Code hook protocol bridge for omp.
 *
 * Makes the Claude hook system (settings.json `hooks` blocks + JSON-on-stdin
 * command scripts) work directly under omp, so repos keep one hook source of
 * truth. Config is read exactly like Claude Code reads it:
 *
 *   ~/.claude/settings.json                  (user;   $CLAUDE_CONFIG_DIR honored)
 *   <project>/.claude/settings.json          (project)
 *   <project>/.claude/settings.local.json    (local)
 *
 * Hook arrays concatenate across levels (Claude semantics: all matching hooks
 * run; identical commands are deduplicated per event fire). Scripts get the
 * Claude wire protocol: JSON payload on stdin, CLAUDE_PROJECT_DIR in env,
 * exit 0 = ok (stdout JSON honored), exit 2 = blocking (stderr is the reason),
 * anything else = non-blocking warning.
 *
 * Event mapping (Claude -> omp extension bus):
 *   SessionStart     -> session_start / session_switch (context injected on
 *                       the next before_agent_start as a visible message)
 *   UserPromptSubmit -> before_agent_start (context injection only; omp has
 *                       no prompt-cancel channel, blocks are surfaced as
 *                       warnings)
 *   PreToolUse       -> tool_call ({ block, reason })
 *   PostToolUse      -> tool_result (feedback appended to result content)
 *   Stop             -> session_stop (omp natively speaks decision:"block")
 *   SubagentStop     -> tool_result of the `task` tool (approximation)
 *   SessionEnd       -> session_shutdown
 *   PreCompact       -> session_before_compact
 *
 * Known gaps (documented, not silently dropped): PreToolUse input mutation
 * (`updatedInput`) is impossible - omp tool_call can only block; Notification
 * hooks have no omp equivalent; omp-only tools (eval, ast_edit, lsp, ...) are
 * exposed under PascalCase passthrough names so `*` matchers still see them.
 *
 * Kill switch: OMP_CLAUDE_HOOKS=0. Inspect state with /claude-hooks.
 */
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import type { ExtensionAPI } from "@oh-my-pi/pi-coding-agent";

// ---------------------------------------------------------------------------
// Boundary guards (settings.json and hook stdout are external input)
// ---------------------------------------------------------------------------

function isRecord(v: unknown): v is Record<string, unknown> {
	return typeof v === "object" && v !== null && !Array.isArray(v);
}

function asString(v: unknown): string | undefined {
	return typeof v === "string" ? v : undefined;
}

function asNumber(v: unknown): number | undefined {
	return typeof v === "number" && Number.isFinite(v) ? v : undefined;
}

function parseJsonObject(text: string): Record<string, unknown> | undefined {
	if (!text.startsWith("{")) return undefined;
	try {
		const parsed: unknown = JSON.parse(text);
		return isRecord(parsed) ? parsed : undefined;
	} catch {
		return undefined;
	}
}

// ---------------------------------------------------------------------------
// Config model
// ---------------------------------------------------------------------------

type HookSource = "user" | "project" | "local";

interface HookEntry {
	event: string;
	matcher: string | undefined;
	command: string;
	timeoutMs: number;
	source: HookSource;
}

interface BridgeConfig {
	cwd: string;
	projectDir: string;
	entries: HookEntry[];
	files: string[];
}

const DEFAULT_TIMEOUT_MS = 60_000; // Claude Code default per hook command

function parseSettingsHooks(raw: string, source: HookSource): HookEntry[] {
	const json = parseJsonObject(raw.trim());
	if (!json || !isRecord(json.hooks)) return [];
	const out: HookEntry[] = [];
	for (const [event, groups] of Object.entries(json.hooks)) {
		if (!Array.isArray(groups)) continue;
		for (const group of groups) {
			if (!isRecord(group) || !Array.isArray(group.hooks)) continue;
			const matcher = asString(group.matcher);
			for (const hook of group.hooks) {
				if (!isRecord(hook) || hook.type !== "command") continue;
				const command = asString(hook.command);
				if (!command) continue;
				const timeoutSec = asNumber(hook.timeout);
				out.push({
					event,
					matcher,
					command,
					timeoutMs: timeoutSec !== undefined ? timeoutSec * 1000 : DEFAULT_TIMEOUT_MS,
					source,
				});
			}
		}
	}
	return out;
}

function findProjectRoot(cwd: string): string {
	let dir = resolve(cwd);
	for (;;) {
		if (existsSync(join(dir, ".git")) || existsSync(join(dir, ".claude"))) return dir;
		const parent = dirname(dir);
		if (parent === dir) return resolve(cwd);
		dir = parent;
	}
}

function loadConfig(cwd: string): BridgeConfig {
	const projectDir = findProjectRoot(cwd);
	const userDir = process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), ".claude");
	const sources: { path: string; source: HookSource }[] = [
		{ path: join(userDir, "settings.json"), source: "user" },
		{ path: join(projectDir, ".claude", "settings.json"), source: "project" },
		{ path: join(projectDir, ".claude", "settings.local.json"), source: "local" },
	];
	const entries: HookEntry[] = [];
	const files: string[] = [];
	for (const src of sources) {
		let raw: string;
		try {
			raw = readFileSync(src.path, "utf8");
		} catch {
			continue;
		}
		files.push(src.path);
		entries.push(...parseSettingsHooks(raw, src.source));
	}
	return { cwd, projectDir, entries, files };
}

// ---------------------------------------------------------------------------
// Tool name / input translation (omp -> Claude vocabulary)
// ---------------------------------------------------------------------------

/** Aliases let matchers written for either Claude generation ("Task" vs "Agent") fire. */
const OMP_TO_CLAUDE_TOOL: Record<string, string[]> = {
	bash: ["Bash"],
	read: ["Read"],
	edit: ["Edit"],
	write: ["Write"],
	grep: ["Grep"],
	glob: ["Glob"],
	task: ["Task", "Agent"],
	todo: ["TodoWrite"],
	web_search: ["WebSearch"],
	browser: ["Browser"],
	ask: ["AskUserQuestion"],
};

function claudeToolNames(ompName: string): string[] {
	const mapped = OMP_TO_CLAUDE_TOOL[ompName];
	if (mapped) return mapped;
	const pascal = ompName
		.split(/[_-]/)
		.map(part => (part ? part[0]!.toUpperCase() + part.slice(1) : part))
		.join("");
	return [pascal];
}

function matcherMatches(matcher: string | undefined, candidates: string[]): boolean {
	if (matcher === undefined || matcher === "" || matcher === "*") return true;
	let re: RegExp | undefined;
	try {
		re = new RegExp(`^(?:${matcher})$`);
	} catch {
		re = undefined;
	}
	if (re !== undefined) {
		const compiled = re;
		return candidates.some(c => compiled.test(c));
	}
	return candidates.includes(matcher);
}

/**
 * Best-effort translation of omp tool inputs into the Claude shapes hook
 * scripts expect (`tool_input.command`, `tool_input.file_path`, ...). The raw
 * omp input always rides along as `tool_input_omp` for precision-hungry hooks.
 */
function translateToolInput(toolName: string, input: Record<string, unknown>): Record<string, unknown> {
	switch (toolName) {
		case "bash":
			return { command: input.command, description: input.i, timeout: input.timeout };
		case "write":
			return { file_path: input.path, content: input.content };
		case "read":
			return { file_path: input.path };
		case "edit": {
			// omp edit input is a hashline patch; recover the target path from
			// the first `[path#TAG]` section header.
			const patch = asString(input.input) ?? "";
			const header = patch.match(/^\[([^\]#]+)#[0-9A-Fa-f]{4}\]/m);
			return { file_path: header?.[1] };
		}
		case "grep":
			return { pattern: input.pattern, path: input.path };
		case "glob":
			return { pattern: input.path };
		case "task": {
			const tasks = Array.isArray(input.tasks) ? input.tasks : [];
			const first = isRecord(tasks[0]) ? tasks[0] : undefined;
			return {
				description: first ? asString(first.description) : undefined,
				prompt: first ? asString(first.assignment) : undefined,
				subagent_type: asString(input.agent) ?? "task",
			};
		}
		default:
			return { ...input };
	}
}

// ---------------------------------------------------------------------------
// Hook execution (Claude wire protocol)
// ---------------------------------------------------------------------------

interface HookRunResult {
	event: string;
	command: string;
	code: number | null;
	stdout: string;
	stderr: string;
	timedOut: boolean;
	spawnError: string | undefined;
	durationMs: number;
}

async function runHookCommand(
	entry: HookEntry,
	payload: Record<string, unknown>,
	projectDir: string,
): Promise<HookRunResult> {
	const started = Date.now();
	const shell = Bun.which("bash") ?? "/bin/sh";
	try {
		const proc = Bun.spawn([shell, "-c", entry.command], {
			cwd: projectDir,
			env: { ...process.env, CLAUDE_PROJECT_DIR: projectDir },
			stdin: "pipe",
			stdout: "pipe",
			stderr: "pipe",
		});
		proc.stdin.write(JSON.stringify(payload));
		proc.stdin.end();
		let timedOut = false;
		const softKill = setTimeout(() => {
			timedOut = true;
			proc.kill();
		}, entry.timeoutMs);
		const hardKill = setTimeout(() => proc.kill(9), entry.timeoutMs + 5000);
		const [stdout, stderr, code] = await Promise.all([
			new Response(proc.stdout).text(),
			new Response(proc.stderr).text(),
			proc.exited,
		]);
		clearTimeout(softKill);
		clearTimeout(hardKill);
		return {
			event: entry.event,
			command: entry.command,
			code,
			stdout,
			stderr,
			timedOut,
			spawnError: undefined,
			durationMs: Date.now() - started,
		};
	} catch (err) {
		return {
			event: entry.event,
			command: entry.command,
			code: null,
			stdout: "",
			stderr: "",
			timedOut: false,
			spawnError: err instanceof Error ? err.message : String(err),
			durationMs: Date.now() - started,
		};
	}
}

// ---------------------------------------------------------------------------
// Result interpretation (Claude exit-code + stdout-JSON semantics)
// ---------------------------------------------------------------------------

interface HookOutcome {
	block: boolean;
	ask: boolean;
	reason: string | undefined;
	context: string[];
	systemMessages: string[];
	warnings: string[];
}

function emptyOutcome(): HookOutcome {
	return { block: false, ask: false, reason: undefined, context: [], systemMessages: [], warnings: [] };
}

function truncate(text: string, max: number): string {
	return text.length > max ? `${text.slice(0, max)}...[truncated]` : text;
}

function shortCommand(command: string): string {
	const firstToken = command.trim().split(/\s+/)[0] ?? command;
	const segments = firstToken.split("/");
	return segments[segments.length - 1] ?? firstToken;
}

/** Events whose plain (non-JSON) stdout becomes model-visible context, per Claude. */
const CONTEXT_STDOUT_EVENTS = new Set(["SessionStart", "UserPromptSubmit"]);

function interpretRun(run: HookRunResult): HookOutcome {
	const outcome = emptyOutcome();
	const name = shortCommand(run.command);
	if (run.spawnError !== undefined) {
		outcome.warnings.push(`${name}: spawn failed (${run.spawnError})`);
		return outcome;
	}
	if (run.timedOut) {
		outcome.warnings.push(`${name}: timed out`);
		return outcome;
	}
	if (run.code === 2) {
		const detail = run.stderr.trim() || run.stdout.trim();
		outcome.block = true;
		outcome.reason = detail || `${name} blocked (exit 2)`;
		return outcome;
	}
	if (run.code !== 0) {
		const detail = run.stderr.trim();
		outcome.warnings.push(`${name}: exit ${run.code}${detail ? `: ${truncate(detail, 200)}` : ""}`);
		return outcome;
	}
	const stdout = run.stdout.trim();
	const json = parseJsonObject(stdout);
	if (!json) {
		if (stdout && CONTEXT_STDOUT_EVENTS.has(run.event)) outcome.context.push(stdout);
		return outcome;
	}
	const systemMessage = asString(json.systemMessage);
	if (systemMessage) outcome.systemMessages.push(systemMessage);
	const hso = isRecord(json.hookSpecificOutput) ? json.hookSpecificOutput : undefined;
	if (hso) {
		const additional = asString(hso.additionalContext);
		if (additional) outcome.context.push(additional);
		const permission = asString(hso.permissionDecision);
		if (permission === "deny") {
			outcome.block = true;
			outcome.reason = asString(hso.permissionDecisionReason) ?? `${name} denied`;
		} else if (permission === "ask") {
			outcome.ask = true;
			outcome.reason = asString(hso.permissionDecisionReason);
		}
	}
	if (!outcome.block && json.decision === "block") {
		outcome.block = true;
		outcome.reason = asString(json.reason) ?? `${name} blocked`;
	}
	if (!outcome.block && json.continue === false) {
		outcome.block = true;
		outcome.reason = asString(json.stopReason) ?? `${name} requested stop`;
	}
	return outcome;
}

function combineOutcomes(outcomes: HookOutcome[]): HookOutcome {
	const combined = emptyOutcome();
	for (const o of outcomes) {
		if (o.block && !combined.block) {
			combined.block = true;
			combined.reason = o.reason;
		}
		if (o.ask && !combined.ask && !combined.block) {
			combined.ask = true;
			combined.reason = combined.reason ?? o.reason;
		}
		combined.context.push(...o.context);
		combined.systemMessages.push(...o.systemMessages);
		combined.warnings.push(...o.warnings);
	}
	return combined;
}

// ---------------------------------------------------------------------------
// Extension
// ---------------------------------------------------------------------------

/** Structural slice of ExtensionContext the bridge needs (keeps helpers testable). */
interface BridgeCtx {
	ui: {
		notify(message: string, type?: "info" | "warning" | "error"): void;
		setStatus(key: string, text: string | undefined): void;
		confirm(title: string, message: string): Promise<boolean | undefined>;
	};
	hasUI: boolean;
	cwd: string;
	sessionManager: {
		getSessionId(): string;
		getSessionFile(): string | undefined;
	};
}

/** Structural slices of the omp event payloads the bridge reads. */
interface PromptEvent {
	prompt: string;
}

interface ToolCallBridgeEvent {
	toolName: string;
	input: unknown;
}

interface ToolResultBridgeEvent {
	toolName: string;
	input: unknown;
	content: { type: string; text?: string }[];
	isError: boolean;
}

interface FireOptions {
	/** Values the entry matcher is tested against (tool names, session source). */
	candidates?: string[];
	/** Event-specific payload fields merged over the common ones. */
	extra?: Record<string, unknown>;
}

/**
 * Main-vs-subagent detection. omp subagents re-bind every extension against a
 * fresh module instance (the loader imports with a `?mtime` cache-buster), so
 * module state cannot distinguish bindings. A globalThis slot survives across
 * instances: the first session to claim it is the main session; a binding
 * seeing a foreign id is a subagent. Claude Code parity: session-level hooks
 * (SessionStart, UserPromptSubmit, Stop, SessionEnd, PreCompact) run only in
 * the main session; PreToolUse/PostToolUse guard the whole agent tree.
 */
interface MainSlot {
	id?: string;
}

const MAIN_SLOT_KEY = "__ompClaudeHooksMainSession";

function mainSlot(): MainSlot {
	// Unchecked cast: globalThis is the only store surviving the loader's
	// per-binding module cache-busting; the key is namespaced and owned here.
	const g = globalThis as unknown as Record<string, MainSlot | undefined>;
	let slot = g[MAIN_SLOT_KEY];
	if (slot === undefined) {
		slot = {};
		g[MAIN_SLOT_KEY] = slot;
	}
	return slot;
}

const CONTEXT_MESSAGE_TYPE = "claude-hook-context";
const MAX_RECENT_RUNS = 50;

export default function claudeHooksBridge(pi: ExtensionAPI) {
	if (process.env.OMP_CLAUDE_HOOKS === "0") return;

	let config: BridgeConfig | undefined;
	let pendingContext: string[] = [];
	let startupRun: Promise<void> = Promise.resolve();
	let role: "main" | "subagent" | undefined;
	const recentRuns: HookRunResult[] = [];

	/** Claim or confirm the main slot; classify this binding once. */
	const resolveRole = (ctx: BridgeCtx): "main" | "subagent" => {
		if (role !== undefined) return role;
		const slot = mainSlot();
		const sid = ctx.sessionManager.getSessionId();
		if (slot.id === undefined || slot.id === sid) {
			slot.id = sid;
			role = "main";
		} else {
			role = "subagent";
		}
		return role;
	};

	const getConfig = (cwd: string): BridgeConfig => {
		if (!config || config.cwd !== cwd) config = loadConfig(cwd);
		return config;
	};

	async function fire(eventName: string, ctx: BridgeCtx, opts: FireOptions = {}): Promise<HookOutcome> {
		const cfg = getConfig(ctx.cwd);
		const candidates = opts.candidates ?? ["*"];
		const matched = cfg.entries.filter(e => e.event === eventName && matcherMatches(e.matcher, candidates));
		if (matched.length === 0) return emptyOutcome();
		const seen = new Set<string>();
		const deduped = matched.filter(e => {
			if (seen.has(e.command)) return false;
			seen.add(e.command);
			return true;
		});
		const payload: Record<string, unknown> = {
			session_id: ctx.sessionManager.getSessionId(),
			transcript_path: ctx.sessionManager.getSessionFile() ?? "",
			cwd: ctx.cwd,
			hook_event_name: eventName,
			...opts.extra,
		};
		ctx.ui.setStatus("claude-hooks", `claude-hooks: ${eventName} (${deduped.length})`);
		try {
			const runs = await Promise.all(deduped.map(entry => runHookCommand(entry, payload, cfg.projectDir)));
			for (const run of runs) {
				recentRuns.push(run);
				if (recentRuns.length > MAX_RECENT_RUNS) recentRuns.shift();
			}
			const outcome = combineOutcomes(runs.map(interpretRun));
			for (const warning of outcome.warnings) ctx.ui.notify(`claude-hooks ${warning}`, "warning");
			for (const message of outcome.systemMessages) ctx.ui.notify(message, "info");
			return outcome;
		} finally {
			ctx.ui.setStatus("claude-hooks", undefined);
		}
	}

	function runSessionStart(ctx: BridgeCtx, source: "startup" | "resume"): void {
		config = loadConfig(ctx.cwd);
		startupRun = (async () => {
			const outcome = await fire("SessionStart", ctx, {
				candidates: [source],
				extra: { source },
			});
			if (outcome.block && outcome.reason) {
				ctx.ui.notify(`claude-hooks SessionStart: ${outcome.reason}`, "warning");
			}
			pendingContext.push(...outcome.context);
		})();
		// Deliberately not awaited: SessionStart hooks may be slow (ix runs
		// `nix run #init` with a 150s budget). before_agent_start awaits, so
		// the first turn is still guaranteed to see the injected context.
	}

	pi.on("session_start", async (_event: unknown, ctx: BridgeCtx) => {
		if (resolveRole(ctx) === "subagent") return; // Claude parity: no SessionStart in subagents
		runSessionStart(ctx, "startup");
	});

	pi.on("session_switch", async (_event: unknown, ctx: BridgeCtx) => {
		if (resolveRole(ctx) === "subagent") return;
		mainSlot().id = ctx.sessionManager.getSessionId(); // follow the main session across /new
		runSessionStart(ctx, "resume");
	});

	pi.on("before_agent_start", async (event: PromptEvent, ctx: BridgeCtx) => {
		if (resolveRole(ctx) === "subagent") return undefined; // Claude parity: no UserPromptSubmit in subagents
		await startupRun;
		const submit = await fire("UserPromptSubmit", ctx, { extra: { prompt: event.prompt } });
		if (submit.block) {
			ctx.ui.notify(
				`claude-hooks: UserPromptSubmit block is unsupported in omp${submit.reason ? ` (${submit.reason})` : ""}`,
				"warning",
			);
		}
		const context = [...pendingContext, ...submit.context];
		pendingContext = [];
		if (context.length === 0) return undefined;
		return {
			message: {
				customType: CONTEXT_MESSAGE_TYPE,
				content: context.join("\n\n"),
				display: true,
			},
		};
	});

	pi.on("tool_call", async (event: ToolCallBridgeEvent, ctx: BridgeCtx) => {
		const candidates = claudeToolNames(event.toolName);
		const input = isRecord(event.input) ? event.input : {};
		const outcome = await fire("PreToolUse", ctx, {
			candidates,
			extra: {
				tool_name: candidates[0],
				tool_input: translateToolInput(event.toolName, input),
				tool_input_omp: input,
			},
		});
		if (outcome.context.length > 0) {
			pi.sendMessage(
				{ customType: CONTEXT_MESSAGE_TYPE, content: outcome.context.join("\n\n"), display: false },
				{ deliverAs: "nextTurn" },
			);
		}
		if (outcome.block) return { block: true, reason: outcome.reason ?? "blocked by Claude hook" };
		if (outcome.ask) {
			const question = outcome.reason ?? `Claude hook requests approval for ${candidates[0]}`;
			if (!ctx.hasUI) return { block: true, reason: `approval required (no UI): ${question}` };
			const approved = await ctx.ui.confirm("Claude hook approval", question);
			if (!approved) return { block: true, reason: `denied: ${question}` };
		}
		return undefined;
	});

	pi.on("tool_result", async (event: ToolResultBridgeEvent, ctx: BridgeCtx) => {
		const candidates = claudeToolNames(event.toolName);
		const input = isRecord(event.input) ? event.input : {};
		const responseText = event.content
			.map(chunk => (chunk.type === "text" ? (chunk.text ?? "") : "[image]"))
			.join("\n");
		const outcome = await fire("PostToolUse", ctx, {
			candidates,
			extra: {
				tool_name: candidates[0],
				tool_input: translateToolInput(event.toolName, input),
				tool_input_omp: input,
				tool_response: { content: truncate(responseText, 100_000), isError: event.isError },
			},
		});
		const feedback: string[] = [];
		if (outcome.block) feedback.push(outcome.reason ?? "post-hook feedback");
		if (event.toolName === "task") {
			const subagent = await fire("SubagentStop", ctx, { extra: { stop_hook_active: false } });
			if (subagent.block) feedback.push(subagent.reason ?? "subagent stop feedback");
			outcome.context.push(...subagent.context);
		}
		if (outcome.context.length > 0) {
			pi.sendMessage(
				{ customType: CONTEXT_MESSAGE_TYPE, content: outcome.context.join("\n\n"), display: false },
				{ deliverAs: "nextTurn" },
			);
		}
		if (feedback.length === 0) return undefined;
		return {
			content: [...event.content, { type: "text", text: `\n[claude-hook feedback]\n${feedback.join("\n")}` }],
		};
	});

	pi.on("session_stop", async (_event: unknown, ctx: BridgeCtx) => {
		if (resolveRole(ctx) === "subagent") return undefined; // Stop is a main-agent concept; SubagentStop maps via task tool_result
		const outcome = await fire("Stop", ctx, { extra: { stop_hook_active: false } });
		if (outcome.block) {
			return { decision: "block" as const, reason: outcome.reason ?? "stop blocked by Claude hook" };
		}
		return undefined;
	});

	pi.on("session_shutdown", async (_event: unknown, ctx: BridgeCtx) => {
		if (resolveRole(ctx) === "subagent") return;
		await fire("SessionEnd", ctx, { extra: { reason: "exit" } });
	});

	pi.on("session_before_compact", async (_event: unknown, ctx: BridgeCtx) => {
		if (resolveRole(ctx) === "subagent") return undefined;
		await fire("PreCompact", ctx, { extra: { trigger: "manual", custom_instructions: "" } });
		return undefined;
	});

	pi.registerCommand("claude-hooks", {
		description: "Show Claude hook bridge state (config + recent runs)",
		handler: async (_args: string, ctx: BridgeCtx) => {
			const cfg = getConfig(ctx.cwd);
			const lines: string[] = [
				`project: ${cfg.projectDir}`,
				`settings: ${cfg.files.length > 0 ? cfg.files.join(", ") : "none found"}`,
				"",
			];
			if (cfg.entries.length === 0) {
				lines.push("no Claude hooks configured");
			} else {
				for (const entry of cfg.entries) {
					lines.push(`${entry.event}${entry.matcher ? `[${entry.matcher}]` : ""} (${entry.source}): ${entry.command}`);
				}
			}
			if (recentRuns.length > 0) {
				lines.push("", "recent runs:");
				for (const run of recentRuns.slice(-15)) {
					const status = run.spawnError ? "spawn-error" : run.timedOut ? "timeout" : `exit ${run.code}`;
					lines.push(`${run.event} ${shortCommand(run.command)} -> ${status} (${run.durationMs}ms)`);
				}
			}
			pi.sendMessage({ customType: "claude-hooks-status", content: lines.join("\n"), display: true });
		},
	});
}
