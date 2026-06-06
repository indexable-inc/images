// Shared helpers for tool-call row rendering. Both ToolWork and
// ShellWork pull from here so the message-shape contract lives in
// one place. Pure TS so unit tests are trivial if we ever want them.

import type { Message, ToolResult } from './types';

export function isPlainObject(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

export function strField(input: unknown, key: string): string | null {
  if (!isPlainObject(input)) return null;
  const v = input[key];
  return typeof v === 'string' && v.length > 0 ? v : null;
}

/** Status of a tool call, or null for non-tool messages. */
export function toolStatus(m: Message): ToolResult['status'] | null {
  return m.result?.status ?? null;
}

/** A tool call still in flight: an explicit `running` result, or a
 *  `tool_call` row that has no result yet and isn't a diff. */
export function isRunning(m: Message): boolean {
  if (m.result) return m.result.status === 'running';
  return m.kind === 'tool_call' && m.patch == null;
}

export function isError(m: Message): boolean {
  return m.result?.status === 'error';
}

/** The failure reason for an errored tool call, else null. */
export function errorMessage(m: Message): string | null {
  return m.result?.status === 'error' ? m.result.message : null;
}

/** The human-readable text of a tool result: the server-computed
 *  `display`, falling back to flattening the structured `content`. The
 *  server already unwraps MCP envelopes, so this is mostly a field read;
 *  `flattenTextual` stays for `content` and any unexpected shape. */
export function toolDisplay(m: Message): string {
  const r = m.result;
  if (!r) return '';
  if ('display' in r && typeof r.display === 'string' && r.display.length > 0) {
    return r.display;
  }
  if ('content' in r && r.content != null) return flattenTextual(r.content);
  return '';
}

export function flattenTextual(v: unknown): string {
  if (v == null) return '';
  if (typeof v === 'string') return v;
  if (Array.isArray(v)) return v.map(flattenTextual).filter(Boolean).join('\n');
  if (isPlainObject(v)) {
    if (typeof v.content === 'string') return v.content;
    if (Array.isArray(v.content)) return flattenTextual(v.content);
    if (typeof v.text === 'string') return v.text;
    return JSON.stringify(v, null, 2);
  }
  return String(v);
}
