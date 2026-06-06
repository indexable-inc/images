// Chat composer client. Submits the user's prompt as a real Codex
// turn; the server records it, dispatches `turn/start` over JSON-RPC,
// and streams the agent's items back through Delta::MessageAppend.
//
// thread_id is server-authoritative: codex assigns the id on the
// first send and the server echoes it back. Pass null for a fresh
// thread; pass the previous id to continue.

import { z } from 'zod';
import type { Identity } from './identity';
import { backendHttpBase } from './backend';

export const ChatInputItem = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('text'),
    text: z.string(),
    textElements: z.array(z.unknown()).optional()
  }),
  z.object({
    type: z.literal('image'),
    url: z.string().startsWith('data:image/'),
    detail: z.string().optional()
  }),
  z.object({
    type: z.literal('localImage'),
    path: z.string(),
    detail: z.string().optional()
  }),
  z.object({
    type: z.literal('skill'),
    name: z.string(),
    path: z.string()
  }),
  z.object({
    type: z.literal('mention'),
    name: z.string(),
    path: z.string()
  })
]);
export type ChatInputItem = z.infer<typeof ChatInputItem>;

export const SendChatRequest = z
  .object({
    thread_id: z.string().nullable(),
    text: z.string().max(16_000).default(''),
    cwd: z.string().optional(),
    author_id: z.string(),
    author_name: z.string(),
    images: z.array(z.string().startsWith('data:image/')).max(8).default([]),
    input: z.array(ChatInputItem).default([]),
    model: z.string().optional(),
    effort: z.string().optional(),
    approval_policy: z.unknown().optional(),
    permissions: z.string().optional()
  })
  .refine((v) => v.text.length > 0 || v.images.length > 0 || v.input.length > 0, {
    message: 'input is empty'
  });
export type SendChatRequest = z.infer<typeof SendChatRequest>;

export const SendChatResponse = z.object({
  thread_id: z.string()
});
export type SendChatResponse = z.infer<typeof SendChatResponse>;

export interface SendChatArgs {
  serverId: string;
  thread_id: string | null;
  text: string;
  author: Identity;
  cwd?: string;
  images?: string[];
  input?: ChatInputItem[];
  model?: string;
  effort?: string;
  approval_policy?: unknown;
  permissions?: string;
}

export async function sendChat(args: SendChatArgs): Promise<SendChatResponse> {
  const body = SendChatRequest.parse({
    thread_id: args.thread_id,
    text: args.text,
    cwd: args.cwd,
    author_id: args.author.id,
    author_name: args.author.name,
    images: args.images ?? [],
    input: args.input ?? [],
    model: args.model,
    effort: args.effort,
    approval_policy: args.approval_policy,
    permissions: args.permissions
  });
  const r = await fetch(backendHttpBase(args.serverId) + '/api/chat', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!r.ok) {
    const body = await r.text().catch(() => '');
    throw new Error(`POST /api/chat -> ${r.status}: ${body}`);
  }
  return SendChatResponse.parse(await r.json());
}
