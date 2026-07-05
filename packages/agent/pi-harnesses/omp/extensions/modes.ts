/**
 * Model modes: named bundles of modelRoles switched with /mode.
 *
 * Definitions are nix-generated into ~/.omp/agent/modes.json (source of truth:
 * modules/users/user-config/agents.nix, `ompModes`). Applying a mode replaces
 * the persisted modelRoles record wholesale - roles from the previous mode
 * never leak through - then live-switches the session model to the mode's
 * `default` pattern. Subagents resolve `roles.task` at spawn time, so a switch
 * affects the next spawn immediately; no restart needed.
 */
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@oh-my-pi/pi-coding-agent";

interface ModeDef {
	description?: string;
	roles: Record<string, string>;
}

type ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh";

const MODES_FILE = join(homedir(), ".omp", "agent", "modes.json");
const THINKING_SUFFIXES: Record<string, ThinkingLevel> = {
	off: "off",
	minimal: "minimal",
	low: "low",
	medium: "medium",
	high: "high",
	xhigh: "xhigh",
	max: "xhigh",
};

/** Fresh read every call: the file is a store symlink that moves on rebuild. */
function loadModes(): Record<string, ModeDef> {
	try {
		return JSON.parse(readFileSync(MODES_FILE, "utf8")) as Record<string, ModeDef>;
	} catch {
		return {};
	}
}

/** Thinking suffix of the first pattern in a role value ("a/b:high,c/d" -> "high"). */
function thinkingOf(pattern: string): ThinkingLevel | undefined {
	const first = pattern.split(",")[0]!.trim().split("@")[0]!;
	const idx = first.lastIndexOf(":");
	if (idx === -1) return undefined;
	return THINKING_SUFFIXES[first.slice(idx + 1)];
}

export default function modesExtension(pi: ExtensionAPI) {
	pi.registerCommand("mode", {
		description: "Switch model mode (named modelRoles bundle)",
		getArgumentCompletions: prefix => {
			const items = Object.entries(loadModes())
				.filter(([name]) => name.startsWith(prefix))
				.map(([name, mode]) => ({ value: name, label: name, description: mode.description }));
			return items.length > 0 ? items : null;
		},
		handler: async (args, ctx) => {
			const modes = loadModes();
			const names = Object.keys(modes);
			if (names.length === 0) {
				ctx.ui.notify(`no modes defined in ${MODES_FILE}`, "error");
				return;
			}

			let name = args.trim();
			if (!name) {
				// Mark the mode whose bundle equals the live modelRoles record.
				const live = pi.pi.settings.getModelRoles();
				const active = names.find(n => {
					const roles = modes[n]!.roles;
					const keys = Object.keys(roles);
					return keys.length === Object.keys(live).length && keys.every(k => roles[k] === live[k]);
				});
				if (!ctx.hasUI) {
					ctx.ui.notify(`modes: ${names.map(n => (n === active ? `${n} (active)` : n)).join(", ")}`);
					return;
				}
				const picked = await ctx.ui.select(
					"Model mode",
					names.map(n => ({
						label: n === active ? `${n} (active)` : n,
						description: modes[n]?.description,
					})),
				);
				if (!picked) return;
				name = picked.replace(/ \(active\)$/, "");
			}

			const mode = modes[name];
			if (!mode) {
				ctx.ui.notify(`unknown mode '${name}' (have: ${names.join(", ")})`, "error");
				return;
			}

			// Resolve the main model up front so a broken bundle is a no-op.
			const defaultPattern = mode.roles.default;
			let model;
			if (defaultPattern) {
				model = ctx.models.resolve(defaultPattern.split(",")[0]!.trim());
				if (!model) {
					ctx.ui.notify(`mode '${name}': cannot resolve '${defaultPattern}'`, "error");
					return;
				}
			}

			// Replace the whole record: roles from the previous mode must not leak.
			pi.pi.settings.set("modelRoles", { ...mode.roles });

			if (model) {
				if (!(await pi.setModel(model))) {
					ctx.ui.notify(`mode '${name}': no credentials for ${model.provider}/${model.id}`, "error");
					return;
				}
				const thinking = thinkingOf(defaultPattern);
				if (thinking) {
					pi.setThinkingLevel(thinking);
				}
			}

			ctx.ui.notify(`mode: ${name}${mode.description ? ` - ${mode.description}` : ""}`);
		},
	});
}
