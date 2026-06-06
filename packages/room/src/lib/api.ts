// REST client for the room server. Pagination uses an `updated_ms`
// cursor exposed by the server's list_threads handler.

import type { Message, Thread } from './types';
import { backendHttpBase } from './backend';

export interface ListThreadsResponse {
  threads: Thread[];
  next_before: number | null;
}

export interface ListMessagesResponse {
  messages: Message[];
}

export interface ListThreadsParams {
  user?: string;
  repo?: string;
  status?: string;
  search?: string;
  limit?: number;
  before?: number;
}

function qs(params: Record<string, string | number | undefined>): string {
  const entries = Object.entries(params).filter(
    ([, v]) => v !== undefined && v !== null && v !== ''
  );
  if (entries.length === 0) return '';
  const u = new URLSearchParams();
  for (const [k, v] of entries) u.set(k, String(v));
  return '?' + u.toString();
}

function url(serverId: string, path: string): string {
  return backendHttpBase(serverId) + path;
}

export async function listThreads(serverId: string, p: ListThreadsParams = {}): Promise<ListThreadsResponse> {
  const r = await fetch(url(serverId, '/api/threads') + qs({ ...p }));
  if (!r.ok) throw new Error(`/api/threads -> ${r.status}`);
  return r.json();
}

export async function getThread(serverId: string, id: string): Promise<Thread | null> {
  const r = await fetch(url(serverId, `/api/threads/${encodeURIComponent(id)}`));
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`/api/threads/${id} -> ${r.status}`);
  return r.json();
}

export async function listMessages(
  serverId: string,
  id: string,
  params: { limit?: number } = {}
): Promise<ListMessagesResponse> {
  const r = await fetch(
    url(serverId, `/api/threads/${encodeURIComponent(id)}/messages`) + qs(params)
  );
  if (!r.ok) throw new Error(`/api/threads/${id}/messages -> ${r.status}`);
  return r.json();
}

export async function archiveThread(serverId: string, id: string): Promise<Thread> {
  const r = await fetch(url(serverId, `/api/threads/${encodeURIComponent(id)}/archive`), {
    method: 'POST'
  });
  if (!r.ok) throw new Error(`/api/threads/${id}/archive -> ${r.status}`);
  return r.json();
}

export async function interruptThread(serverId: string, id: string): Promise<void> {
  const r = await fetch(url(serverId, `/api/threads/${encodeURIComponent(id)}/interrupt`), {
    method: 'POST'
  });
  if (!r.ok) {
    const body = await r.text().catch(() => '');
    throw new Error(`/api/threads/${id}/interrupt -> ${r.status}: ${body}`);
  }
}

/** Set or replace the thread's goal. Codex echoes the change through
 *  a `thread/goal/updated` notification that the bridge persists and
 *  the WS broadcasts as a ThreadUpsert — the response itself is just
 *  an ack, so we don't read its body. */
export async function setThreadGoal(
  serverId: string,
  id: string,
  objective: string,
  tokenBudget: number | null = null
): Promise<void> {
  const r = await fetch(url(serverId, `/api/threads/${encodeURIComponent(id)}/goal`), {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ objective, token_budget: tokenBudget })
  });
  if (!r.ok) {
    const body = await r.text().catch(() => '');
    throw new Error(`/api/threads/${id}/goal -> ${r.status}: ${body}`);
  }
}

/** Clear the thread's goal. Same notification round-trip as
 *  `setThreadGoal`. */
export async function clearThreadGoal(serverId: string, id: string): Promise<void> {
  const r = await fetch(url(serverId, `/api/threads/${encodeURIComponent(id)}/goal`), {
    method: 'DELETE'
  });
  if (!r.ok) {
    const body = await r.text().catch(() => '');
    throw new Error(`/api/threads/${id}/goal DELETE -> ${r.status}: ${body}`);
  }
}

export async function respondCodexRequest(
  serverId: string,
  requestId: string,
  result: unknown
): Promise<void> {
  const r = await fetch(
    url(serverId, `/api/codex/requests/${encodeURIComponent(requestId)}/respond`),
    {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ result })
    }
  );
  if (!r.ok) {
    const body = await r.text().catch(() => '');
    throw new Error(`/api/codex/requests/${requestId}/respond -> ${r.status}: ${body}`);
  }
}

export interface CodexModel {
  id: string;
  model: string;
  displayName: string;
  description: string;
  supportedReasoningEfforts: Array<{ reasoningEffort: string; description: string }>;
  defaultReasoningEffort: string;
  isDefault: boolean;
}

export interface CodexModelsResponse {
  data: CodexModel[];
  nextCursor?: string | null;
}

export async function listCodexModels(serverId: string): Promise<CodexModelsResponse> {
  const r = await fetch(url(serverId, '/api/codex/models'));
  if (!r.ok) throw new Error('/api/codex/models -> ' + r.status);
  return r.json();
}

export interface PermissionProfile {
  id: string;
  description?: string | null;
}

export interface PermissionProfilesResponse {
  data: PermissionProfile[];
  nextCursor?: string | null;
}

export async function listPermissionProfiles(serverId: string, cwd?: string | null): Promise<PermissionProfilesResponse> {
  const r = await fetch(url(serverId, '/api/codex/permission-profiles') + qs({ cwd: cwd ?? undefined }));
  if (!r.ok) throw new Error('/api/codex/permission-profiles -> ' + r.status);
  return r.json();
}

export interface CodexConfigResponse {
  config: {
    model?: string | null;
    model_reasoning_effort?: string | null;
    approval_policy?: unknown | null;
    service_tier?: string | null;
  };
}

export async function readCodexConfig(serverId: string, cwd?: string | null): Promise<CodexConfigResponse> {
  const r = await fetch(url(serverId, '/api/codex/config') + qs({ cwd: cwd ?? undefined }));
  if (!r.ok) throw new Error('/api/codex/config -> ' + r.status);
  return r.json();
}

export interface CodexSkill {
  name: string;
  description: string;
  shortDescription?: string | null;
  path: string;
  enabled: boolean;
}

export interface CodexSkillEntry {
  cwd: string;
  skills: CodexSkill[];
}

export interface CodexSkillsResponse {
  data: CodexSkillEntry[];
}

export async function listCodexSkills(serverId: string, cwd?: string | null): Promise<CodexSkill[]> {
  const r = await fetch(url(serverId, '/api/codex/skills') + qs({ cwd: cwd ?? undefined }));
  if (!r.ok) throw new Error('/api/codex/skills -> ' + r.status);
  const body = (await r.json()) as CodexSkillsResponse;
  return body.data.flatMap((entry) => entry.skills).filter((skill) => skill.enabled);
}

export interface FileSearchResult {
  root: string;
  path: string;
  match_type?: string;
  file_name: string;
  score: number;
  indices?: number[] | null;
}

export async function searchFiles(serverId: string, query: string, cwd?: string | null): Promise<FileSearchResult[]> {
  const r = await fetch(url(serverId, '/api/codex/file-search'), {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ query, cwd })
  });
  if (!r.ok) throw new Error('/api/codex/file-search -> ' + r.status);
  const body = (await r.json()) as { files?: FileSearchResult[] };
  return body.files ?? [];
}

export interface WorkspaceInfo {
  cwd: string;
  root: string;
  repo: string | null;
  branch: string | null;
  base_sha: string | null;
}

export async function getThreadWorkspace(serverId: string, id: string): Promise<WorkspaceInfo> {
  const r = await fetch(url(serverId, '/api/threads/' + encodeURIComponent(id) + '/workspace'));
  if (!r.ok) throw new Error('/api/threads/' + id + '/workspace -> ' + r.status);
  return r.json();
}

export interface ChangedFile {
  path: string;
  status: string;
  additions: number | null;
  deletions: number | null;
}

export async function listChangedFiles(serverId: string, id: string): Promise<ChangedFile[]> {
  const r = await fetch(url(serverId, '/api/threads/' + encodeURIComponent(id) + '/changed-files'));
  if (!r.ok) throw new Error('/api/threads/' + id + '/changed-files -> ' + r.status);
  const body = (await r.json()) as { files: ChangedFile[] };
  return body.files;
}

export async function getThreadDiff(serverId: string, id: string, path?: string | null): Promise<string> {
  const r = await fetch(
    url(serverId, '/api/threads/' + encodeURIComponent(id) + '/diff') + qs({ path: path ?? undefined })
  );
  if (!r.ok) throw new Error('/api/threads/' + id + '/diff -> ' + r.status);
  const body = (await r.json()) as { diff: string };
  return body.diff;
}

export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
}

export interface FileListing {
  root: string;
  path: string;
  entries: FileEntry[];
}

export async function listThreadFiles(serverId: string, id: string, path?: string | null): Promise<FileListing> {
  const r = await fetch(
    url(serverId, '/api/threads/' + encodeURIComponent(id) + '/files') + qs({ path: path ?? undefined })
  );
  if (!r.ok) throw new Error('/api/threads/' + id + '/files -> ' + r.status);
  return r.json();
}

export interface FileContents {
  root: string;
  path: string;
  contents: string;
  truncated: boolean;
}

export async function readThreadFile(serverId: string, id: string, path: string): Promise<FileContents> {
  const r = await fetch(
    url(serverId, '/api/threads/' + encodeURIComponent(id) + '/file') + qs({ path })
  );
  if (!r.ok) throw new Error('/api/threads/' + id + '/file -> ' + r.status);
  return r.json();
}
