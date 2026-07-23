/**
 * claude-agents: make Claude Code agent definitions available to omp's task
 * tool, keeping `.claude/agents/*.md` as the single source of truth.
 *
 * omp intentionally skips `.claude/agents` in task-agent discovery (frontmatter
 * schema mismatch) and only reads `<project>/.omp/agents` and
 * `~/.omp/agent/agents`. Discovery re-scans at every spawn, so materializing
 * files there is enough - there is no in-memory registration API.
 *
 * Strategy (cheapest faithful mapping, chosen after auditing real defs):
 *   - omp lowercases tool names at parse time, so `Read, Grep, Glob, Bash,
 *     Edit, Write, Task` already work. A definition whose frontmatter parses
 *     clean is exposed as a plain SYMLINK - no copy, no drift.
 *   - A definition that needs help (multi-word Claude tools like WebSearch,
 *     Agent->task, model pins like `sonnet`/`inherit`) gets a generated COPY
 *     where only the `tools:`/`model:` frontmatter lines are spliced; the body
 *     is byte-identical. `model:` values without a `/` are dropped so those
 *     agents fall through to the configured task role.
 *
 * Generated artifacts are marked (`x-claude-agents-bridge` frontmatter key for
 * copies; symlink target inside `.claude/agents` for links) and GC'd when the
 * source disappears. Hand-written files in the target dirs are never touched.
 * A `.omp/.gitignore` is dropped only when this extension creates `.omp/`.
 *
 * Runs once per process from the main session (subagent extension re-binding
 * is detected via a globalThis slot - the loader cache-busts module instances,
 * so module state cannot be used). Kill switch: OMP_CLAUDE_AGENTS=0.
 */
import { existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, readlinkSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import type { ExtensionAPI } from "@oh-my-pi/pi-coding-agent";

// ---------------------------------------------------------------------------
// omp tool vocabulary (mirror of src/tools/builtin-names.ts, lowercase)
// ---------------------------------------------------------------------------

const OMP_TOOLS: Record<string, true> = {
	read: true,
	bash: true,
	edit: true,
	ast_grep: true,
	ast_edit: true,
	ask: true,
	debug: true,
	eval: true,
	ssh: true,
	github: true,
	glob: true,
	grep: true,
	lsp: true,
	inspect_image: true,
	browser: true,
	checkpoint: true,
	rewind: true,
	task: true,
	job: true,
	irc: true,
	todo: true,
	web_search: true,
	search_tool_bm25: true,
	write: true,
	memory_edit: true,
	retain: true,
	recall: true,
	reflect: true,
	learn: true,
	manage_skill: true,
	yield: true,
};

/** Claude tool name (lowercased) -> omp tool name. */
const CLAUDE_TOOL_ALIASES: Record<string, string> = {
	websearch: "web_search",
	webfetch: "read",
	agent: "task",
	todowrite: "todo",
	taskcreate: "todo",
	taskupdate: "todo",
	tasklist: "todo",
	notebookedit: "edit",
	notebookread: "read",
	askuserquestion: "ask",
};

const MARKER_KEY = "x-claude-agents-bridge";

// ---------------------------------------------------------------------------
// Frontmatter splicing (line-oriented; body bytes are never touched)
// ---------------------------------------------------------------------------

interface SplitDoc {
	/** Frontmatter lines, without the `---` fences. */
	fmLines: string[];
	/** Everything after the closing fence, verbatim (including leading newline). */
	body: string;
}

function splitFrontmatter(content: string): SplitDoc | undefined {
	const lines = content.split("\n");
	if (lines[0]?.trim() !== "---") return undefined;
	for (let i = 1; i < lines.length; i++) {
		if (lines[i]?.trim() === "---") {
			return { fmLines: lines.slice(1, i), body: lines.slice(i + 1).join("\n") };
		}
	}
	return undefined;
}

/** Tool names from a `tools:` CSV value or a dash-list following the key. */
function collectTools(fmLines: string[], keyIndex: number): { names: string[]; consumed: number } {
	const value = fmLines[keyIndex]!.slice(fmLines[keyIndex]!.indexOf(":") + 1).trim();
	if (value !== "") {
		return { names: value.split(",").map(t => t.trim()).filter(Boolean), consumed: 1 };
	}
	const names: string[] = [];
	let consumed = 1;
	for (let i = keyIndex + 1; i < fmLines.length; i++) {
		const item = fmLines[i]!.match(/^\s+-\s+(.+)$/);
		if (!item) break;
		names.push(item[1]!.trim());
		consumed++;
	}
	return { names, consumed };
}

interface TranslationResult {
	/** True when the source parses correctly under omp as-is (symlink is enough). */
	clean: boolean;
	/** Rewritten full file content; only set when `clean` is false. */
	content?: string;
}

function translate(content: string, sourcePath: string): TranslationResult | undefined {
	const doc = splitFrontmatter(content);
	if (!doc) return undefined;

	let dirty = false;
	const out: string[] = [];
	for (let i = 0; i < doc.fmLines.length; i++) {
		const line = doc.fmLines[i]!;
		if (/^tools\s*:/.test(line)) {
			const { names, consumed } = collectTools(doc.fmLines, i);
			const mapped: string[] = [];
			for (const name of names) {
				const lower = name.toLowerCase();
				const alias = CLAUDE_TOOL_ALIASES[lower];
				if (alias !== undefined) {
					dirty = true;
					if (!mapped.includes(alias)) mapped.push(alias);
				} else {
					// Known tools case-fold on omp's side; unknown names (e.g.
					// mcp__*) pass through untouched - omp ignores unmatched ids.
					if (!mapped.includes(name)) mapped.push(name);
					if (OMP_TOOLS[lower] === undefined && !name.startsWith("mcp__")) dirty = true;
				}
			}
			if (consumed > 1) dirty = true; // normalize dash-lists to CSV
			out.push(`tools: ${mapped.join(", ")}`);
			i += consumed - 1;
			continue;
		}
		if (/^model\s*:/.test(line)) {
			const value = line.slice(line.indexOf(":") + 1).trim();
			if (!value.includes("/")) {
				// Claude aliases (sonnet/opus/haiku/inherit) don't name an omp
				// provider pattern; drop so the agent uses the task model role.
				dirty = true;
				continue;
			}
		}
		out.push(line);
	}

	if (!dirty) return { clean: true };
	out.push(`${MARKER_KEY}: "${sourcePath}"`);
	return { clean: false, content: `---\n${out.join("\n")}\n---\n${doc.body}` };
}

// ---------------------------------------------------------------------------
// Materialization and GC
// ---------------------------------------------------------------------------

function isBridgeSymlink(path: string, sourceDir: string): boolean {
	try {
		const target = resolve(dirname(path), readlinkSync(path));
		return target.startsWith(sourceDir);
	} catch {
		return false;
	}
}

function isBridgeCopy(path: string): boolean {
	try {
		return readFileSync(path, "utf8").slice(0, 4096).includes(`${MARKER_KEY}:`);
	} catch {
		return false;
	}
}

interface SyncStats {
	linked: number;
	rewritten: number;
	removed: number;
	skipped: string[];
}

/** Mirror one Claude agents dir into one omp agents dir. Idempotent. */
function syncAgentsDir(sourceDir: string, targetDir: string, onCreateOmpDir?: () => void): SyncStats {
	const stats: SyncStats = { linked: 0, rewritten: 0, removed: 0, skipped: [] };
	const sources = existsSync(sourceDir)
		? readdirSync(sourceDir).filter((f: string) => f.endsWith(".md"))
		: [];

	// GC pass: drop bridge-owned entries whose source vanished.
	if (existsSync(targetDir)) {
		for (const entry of readdirSync(targetDir)) {
			const full = join(targetDir, entry);
			const ours = lstatSync(full).isSymbolicLink() ? isBridgeSymlink(full, sourceDir) : isBridgeCopy(full);
			if (ours && !sources.includes(entry)) {
				rmSync(full);
				stats.removed++;
			}
		}
	}

	if (sources.length === 0) return stats;
	if (!existsSync(targetDir)) {
		onCreateOmpDir?.();
		mkdirSync(targetDir, { recursive: true });
	}

	for (const file of sources) {
		const sourcePath = join(sourceDir, file);
		const targetPath = join(targetDir, file);
		let content: string;
		try {
			content = readFileSync(sourcePath, "utf8");
		} catch {
			continue;
		}
		const result = translate(content, sourcePath);
		if (!result) {
			stats.skipped.push(`${file}: no frontmatter`);
			continue;
		}

		// Never clobber a hand-written entry: keep regular files without our
		// marker and symlinks that do not point into the Claude agents dir.
		{
			const existing = lstatSyncSafe(targetPath);
			if (existing) {
				const foreign = existing.isSymbolicLink()
					? !isBridgeSymlink(targetPath, sourceDir)
					: !isBridgeCopy(targetPath);
				if (foreign) {
					stats.skipped.push(`${file}: hand-written file exists`);
					continue;
				}
			}
		}

		if (result.clean) {
			const linkTarget = relative(targetDir, sourcePath);
			const existing = lstatSyncSafe(targetPath);
			if (existing?.isSymbolicLink() && readlinkSync(targetPath) === linkTarget) continue;
			if (existing) rmSync(targetPath);
			symlinkSync(linkTarget, targetPath);
			stats.linked++;
		} else if (result.content !== undefined) {
			const existing = lstatSyncSafe(targetPath);
			if (existing && !existing.isSymbolicLink() && readFileSync(targetPath, "utf8") === result.content) continue;
			if (existing) rmSync(targetPath);
			writeFileSync(targetPath, result.content);
			stats.rewritten++;
		}
	}
	return stats;
}

interface LstatLike {
	isSymbolicLink(): boolean;
}

function lstatSyncSafe(path: string): LstatLike | undefined {
	try {
		return lstatSync(path);
	} catch {
		return undefined;
	}
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

// ---------------------------------------------------------------------------
// Extension
// ---------------------------------------------------------------------------

interface MainSlot {
	id?: string;
}

const MAIN_SLOT_KEY = "__ompClaudeAgentsMainSession";

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

interface AgentsBridgeCtx {
	ui: { notify(message: string, type?: "info" | "warning" | "error"): void };
	cwd: string;
	sessionManager: { getSessionId(): string };
}

export default function claudeAgentsBridge(pi: ExtensionAPI) {
	if (process.env.OMP_CLAUDE_AGENTS === "0") return;

	const sync = (ctx: AgentsBridgeCtx): void => {
		const slot = mainSlot();
		const sid = ctx.sessionManager.getSessionId();
		if (slot.id !== undefined && slot.id !== sid) return; // subagent binding
		slot.id = sid;

		try {
			const projectDir = findProjectRoot(ctx.cwd);
			const userClaude = process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), ".claude");
			const agentDir = process.env.PI_CODING_AGENT_DIR ?? join(homedir(), ".omp", "agent");

			const ompDir = join(projectDir, ".omp");
			const hadOmpDir = existsSync(ompDir);
			const project = syncAgentsDir(join(projectDir, ".claude", "agents"), join(ompDir, "agents"), () => {
				if (!hadOmpDir) {
					mkdirSync(ompDir, { recursive: true });
					// Self-ignoring: with only bridge artifacts inside, `git status` stays clean.
					writeFileSync(join(ompDir, ".gitignore"), "agents/\n.gitignore\n");
				}
			});
			const user = syncAgentsDir(join(userClaude, "agents"), join(agentDir, "agents"));

			const changed = project.linked + project.rewritten + project.removed + user.linked + user.rewritten + user.removed;
			const skipped = [...project.skipped, ...user.skipped];
			if (changed > 0) {
				ctx.ui.notify(
					`claude-agents: ${project.linked + user.linked} linked, ${project.rewritten + user.rewritten} translated, ${project.removed + user.removed} removed`,
					"info",
				);
			}
			for (const s of skipped) ctx.ui.notify(`claude-agents: skipped ${s}`, "warning");
		} catch (err) {
			ctx.ui.notify(`claude-agents: sync failed (${err instanceof Error ? err.message : String(err)})`, "warning");
		}
	};

	pi.on("session_start", async (_event: unknown, ctx: AgentsBridgeCtx) => {
		sync(ctx);
	});

	pi.on("session_switch", async (_event: unknown, ctx: AgentsBridgeCtx) => {
		sync(ctx);
	});
}
