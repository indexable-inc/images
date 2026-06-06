// Runtime validation of the room-server wire, beside the hand-written
// types in types.ts. zod is a gate: a frame that fails the schema is a
// version skew or a corrupt frame, so the caller drops it and logs
// rather than let a malformed event throw deep inside a store update.
//
// On a successful parse the caller uses the original frame as-is: the
// schema only validates, it never reshapes. Object schemas are `.loose()`
// so a field the server adds later doesn't fail an older client.

import { z } from 'zod';

const ToolResultSchema = z
  .discriminatedUnion('status', [
    z.object({ status: z.literal('running'), display: z.string().nullish() }),
    z.object({
      status: z.literal('ok'),
      display: z.string().nullish(),
      content: z.unknown().optional(),
      exit_code: z.number().nullish(),
      duration_ms: z.number().nullish()
    }),
    z.object({ status: z.literal('empty'), duration_ms: z.number().nullish() }),
    z.object({
      status: z.literal('error'),
      message: z.string(),
      display: z.string().nullish(),
      content: z.unknown().optional(),
      exit_code: z.number().nullish(),
      duration_ms: z.number().nullish()
    }),
    z.object({ status: z.literal('cancelled') })
  ])
  // The server omits `result` for non-tool messages (serde skips None).
  .nullish();

const MessageSchema = z
  .object({
    id: z.string(),
    thread_id: z.string(),
    ts_ms: z.number(),
    role: z.string(),
    kind: z.string(),
    text: z.string().nullable(),
    tool_name: z.string().nullable(),
    tool_use_id: z.string().nullable(),
    tool_input: z.unknown().nullable(),
    result: ToolResultSchema,
    patch: z.string().nullable(),
    images: z.array(z.string()).optional()
  })
  .loose();

// Thread carries many server-shaped fields with their own null/omit
// rules; validate only that it is an object with an id and let the rest
// pass through untouched.
const ThreadSchema = z.object({ id: z.string() }).loose();

export const ServerEventSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('bootstrap'), threads: z.array(ThreadSchema) }),
  z.object({ type: z.literal('thread-upsert'), thread: ThreadSchema }),
  z.object({ type: z.literal('message-append'), thread_id: z.string(), message: MessageSchema }),
  z.object({ type: z.literal('message-update'), thread_id: z.string(), message: MessageSchema }),
  z.object({ type: z.literal('thread-archive'), thread_id: z.string() }),
  z.object({ type: z.literal('ping') })
]);
