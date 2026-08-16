defmodule IxMcp.Inbox.Announce do
  @moduledoc """
  The `IxMcp.Inbox.Renderer` for inbound-message feeds: one channel line per
  new message, whatever the source. It is also the default renderer, so a
  source that names none is a chat feed.

  Both feeds -- `IxMcp.Inbox.Beeper` over Beeper Desktop's bridged chats,
  `IxMcp.Inbox.Mail` over the signed-in mailbox -- normalize a message into
  `t:item/0` and hand it here, so the wire shape stays identical across
  sources and a client can filter on `meta.source` without knowing how the
  message was fetched.

  Two rules the wire imposes, both easy to get wrong at a call site. `meta`
  values become attributes on the `<channel>` tag the client injects, so
  every value must be a short string -- `IxMcp.MCP.Notifier.channel/2`
  raises on anything else, deliberately, at the producer's call site rather
  than silently dropping the event. And the body is one line: a chat message
  carries newlines and a mail snippet carries runs of whitespace, so
  `preview/2` flattens both before they can break the rendering.

  Every field a source reports is treated as possibly missing. A bridge
  that hands over a message with no sender name is a bad line, not a dead
  feed, so this module substitutes rather than raises: the alternative is a
  `Notifier` ArgumentError killing the watcher mid-sweep and losing the
  whole batch.

  Message CONTENT crosses this module, so nothing here logs. A
  `Logger.warning` carrying a preview would copy a private conversation
  into an on-disk kernel log that outlives the session.
  """

  @behaviour IxMcp.Inbox.Renderer

  alias IxMcp.MCP.Notifier

  @preview_chars 80
  @context_chars 40

  @typedoc """
  One inbound message, normalized by an `IxMcp.Inbox.Source`.

    * `:id` - the source's own message id, opaque. The dedup key, and the
      handle a follow-up read needs (`IxMcp.Gmail.show/1`, a chat fetch).
    * `:platform` - what the reader recognizes the message BY: the bridged
      network ("Signal", "WhatsApp", "iMessage") or "email".
    * `:sender` - display name or address, as the source reports it.
    * `:context` - chat title or mail subject. Dropped from the line when it
      merely repeats `:sender`, which is the normal case for a 1:1 chat
      titled after the person.
    * `:preview` - the message text, or `nil` for an attachment-only message.
  """
  @type item :: %{
          id: String.t(),
          platform: String.t() | nil,
          sender: String.t() | nil,
          context: String.t() | nil,
          preview: String.t() | nil
        }

  @doc """
  Push one message onto the channel.

  `source` is the feed's short label ("beeper", "mail") and reaches the
  client as `meta.source`. The message id rides along so a session that
  wants the rest of the conversation has the handle to fetch it.
  """
  @impl true
  @spec announce(String.t(), item()) :: :ok
  def announce(source, item) when is_binary(source) do
    Notifier.channel(line(item), %{
      "source" => source,
      "platform" => present(item[:platform]) || "chat",
      "sender" => present(item[:sender]) || "unknown sender",
      "id" => to_string(item[:id])
    })
  end

  @doc """
  Say that a sweep found more messages than its limit, without naming them.

  Both services answer newest-first, so a cap drops the OLDEST unseen
  messages. Dropping them silently is indistinguishable from a quiet inbox,
  which is the one thing this feed exists to prevent.
  """
  @impl true
  @spec announce_overflow(String.t(), pos_integer()) :: :ok
  def announce_overflow(source, shown) when is_binary(source) and is_integer(shown) do
    Notifier.channel(
      "#{source}: more new messages arrived than this sweep's limit of #{shown}; " <>
        "the oldest of them were not announced",
      %{"source" => source, "overflow" => "true"}
    )
  end

  @doc """
  One line, at most `cap` graphemes: whitespace runs collapse to single
  spaces and the tail becomes an ellipsis.

  `nil` and whitespace-only text render as a marker rather than an empty
  line -- an attachment-only message should still read as a message.
  """
  @spec preview(String.t() | nil, pos_integer()) :: String.t()
  def preview(text, cap \\ @preview_chars)

  def preview(text, cap) when is_binary(text) and is_integer(cap) do
    flat = String.trim(String.replace(text, ~r/\s+/u, " "))

    cond do
      flat == "" -> "(no text)"
      String.length(flat) <= cap -> flat
      true -> String.slice(flat, 0, cap) <> "..."
    end
  end

  def preview(_text, _cap), do: "(no text)"

  @spec line(item()) :: String.t()
  defp line(item) do
    sender = present(item[:sender]) || "unknown sender"
    platform = present(item[:platform]) || "chat"

    "#{platform} - #{sender}#{context(item[:context], sender)}: " <>
      preview(item[:preview], @preview_chars)
  end

  # A 1:1 chat is titled after the person who is already named as the
  # sender; repeating it costs a third of the line and says nothing.
  defp context(context, sender) do
    case present(context) do
      nil -> ""
      ^sender -> ""
      title -> " (#{preview(title, @context_chars)})"
    end
  end

  defp present(value) when is_binary(value) do
    case String.trim(value) do
      "" -> nil
      trimmed -> trimmed
    end
  end

  defp present(_value), do: nil
end
