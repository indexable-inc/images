defmodule IxMcp.Inbox.Renderer do
  @moduledoc """
  How a feed's items reach the channel, so that `IxMcp.Inbox.Watcher` can own
  the polling loop for feeds whose lines look nothing alike.

  The loop and the line format are separate concerns and were tangled while
  every feed was an inbox: the watcher called
  `IxMcp.Inbox.Announce` directly, which renders a chat message
  ("Signal - Alice (Group): text"). A CI verdict has no sender and no
  preview, so a feed like `IxMcp.Forge.Verdicts` would have had to lie about
  its fields to reuse the loop, or copy the loop to keep its own line -- and
  the loop is where the subtle bugs live (a watermark that advances past
  items nobody heard, a retry storm against a service that is merely
  asleep). Naming a renderer is the third option: one loop, one line format
  per feed, no duplication of either.

  A source names its renderer with `c:IxMcp.Inbox.Source.renderer/0`.
  Sources that do not are chat feeds and get `IxMcp.Inbox.Announce`.

  Both callbacks take the feed's short label as their first argument rather
  than reading it from the source module, because the label is also the
  `meta.source` a client filters on, and a test needs to be able to hand
  over a fixture label instead.
  """

  @doc """
  Push one item onto the channel as one line.

  The item is whatever the paired `IxMcp.Inbox.Source` produced; the loop
  reads only its `:id`. `IxMcp.MCP.Notifier.channel/2` raises on a meta
  value that is not a short string, deliberately, so a renderer normalizes
  here rather than letting the producer's call site fail late.
  """
  @callback announce(String.t(), map()) :: :ok

  @doc """
  Say that a sweep found more items than its limit, without naming them.

  Silently dropping the overflow is indistinguishable from a quiet window,
  which is the one thing a push feed exists to prevent.
  """
  @callback announce_overflow(String.t(), pos_integer()) :: :ok
end
