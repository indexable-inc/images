import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { buildMcpEnv } from "./env.js";

// Bridge the ix-mcp Python-execution MCP server into Pi as native tools.
//
// Pi ships no built-in MCP client, so we run `ix-mcp serve` over stdio, list its
// tools once at startup, and re-expose each through pi.registerTool. The harness
// launches Pi with --no-builtin-tools, so these bridged tools are the ONLY tools
// the model sees: shell, file IO and HTTP all happen inside the shared IPython
// kernel via `python_exec`. This mirrors the Claude Code single-surface posture
// (index/packages/agent/claude-code/default.nix), except Pi makes the built-ins
// genuinely absent rather than relying on permission denies.
//
// Each Pi run spawns its own `ix-mcp serve`, so it gets its own kernel. Shared
// hc2 kernels are a later ticket (ENG-2264).
export default async function (pi: ExtensionAPI): Promise<void> {
  // The harness puts ix-mcp on PATH; IX_MCP_BIN lets the smoke test point at a
  // freshly nix-built binary without a PATH dance.
  const command = process.env.IX_MCP_BIN ?? "ix-mcp";

  const client = new Client({ name: "ix-mcp-bridge", version: "0.1.0" });
  await client.connect(
    new StdioClientTransport({ command, args: ["serve"], env: buildMcpEnv() }),
  );

  const { tools } = await client.listTools();
  for (const tool of tools) {
    pi.registerTool({
      name: tool.name,
      description: tool.description ?? "",
      // MCP inputSchema is JSON Schema; Pi accepts a JSON-Schema-shaped object
      // for `parameters` (Typebox schemas are JSON Schema at runtime), so the
      // MCP schema passes through unchanged.
      parameters: tool.inputSchema as never,
      async execute(_toolCallId, params) {
        const result = await client.callTool({
          name: tool.name,
          arguments: params as Record<string, unknown>,
        });
        // MCP content blocks line up 1:1 with Pi tool-result content blocks.
        return {
          content: (result.content ?? []) as never,
          details: { isError: result.isError ?? false },
        };
      },
    });
  }

  // One ix-mcp (and one kernel) per Pi run: tear it down with the session.
  pi.on("session_shutdown", async () => {
    await client.close();
  });
}
