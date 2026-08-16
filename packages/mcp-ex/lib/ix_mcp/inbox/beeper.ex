defmodule IxMcp.Inbox.Beeper do
  @moduledoc """
  The Beeper Desktop feed: a message arriving on any bridged network
  (Signal, WhatsApp, iMessage, Instagram, Discord, X, ...) becomes one
  channel line.

  Beeper Desktop serves a local REST API on loopback while the app runs,
  authorized by a token the app mints. There is no subscribe endpoint --
  its own `/v1/spec` lists no stream and no webhook -- so this polls
  `GET /v1/messages/search`, which is the right endpoint for three reasons:
  `sender=others` drops the user's own messages server-side, `dateAfter`
  takes the watermark directly, and one call covers every bridged account
  instead of one call per chat.

  ## Two traps in that API, both verified against a live instance

  Items come back NEWEST first, and a page is anchored at the newest end of
  the window rather than adjacent to the cursor: asking for 2 messages
  "after" a cursor 8 messages back returns the newest 2, not the next 2. So
  a cap drops the OLDEST unseen messages, which is why `fetch/3` reports
  overflow rather than letting a busy window look quiet.

  An empty page returns `newestCursor: null`. A loop that followed the
  server's opaque cursor would lose its place on the first quiet minute, so
  the watermark here is the timestamp the watcher owns. That has a second,
  better property: a bridge that reconnects and backfills days of history
  announces nothing, because backfilled messages carry their ORIGINAL
  timestamps and fall outside the window.

  Reads only. Nothing here sends, marks read, or writes.

  ## Configuration (none required)

    * absent, silently, when the token file does not exist -- which is what
      makes the feed safe to default on
    * `IX_MCP_BEEPER_WATCH=0` turns the feed off
    * `IX_MCP_BEEPER_TOKEN_FILE` overrides the documented token path
    * `IX_MCP_BEEPER_URL` overrides the documented local endpoint
    * `IX_MCP_BEEPER_WATCH_INTERVAL_MS` overrides the 5s cadence
    * `IX_MCP_BEEPER_WATCH_QUIET=1` respects the user's own Muted and Low
      Priority marks. The default announces everything, because "anyone who
      has sent a message on any platform" is what the feed was asked for;
      the knob is here for whoever finds that too loud.
  """

  @behaviour IxMcp.Inbox.Source

  alias IxMcp.Inbox.Source

  # Beeper Desktop's documented local endpoint and token path, not anything
  # specific to one machine.
  @default_url "http://127.0.0.1:23373"
  @default_token_file "~/.config/beeper/token"
  @default_interval_ms 5_000
  @timeout_ms 10_000
  @path "/v1/messages/search"

  @impl true
  def label, do: "beeper"

  @impl true
  def default_interval_ms do
    Source.interval_from_env("IX_MCP_BEEPER_WATCH_INTERVAL_MS", @default_interval_ms)
  end

  @impl true
  def init(opts) do
    with true <- System.get_env("IX_MCP_BEEPER_WATCH") != "0",
         {:ok, token} <- token(opts) do
      {:ok,
       %{
         url: Keyword.get(opts, :url, System.get_env("IX_MCP_BEEPER_URL") || @default_url),
         token: token,
         quiet?: Keyword.get(opts, :quiet?, System.get_env("IX_MCP_BEEPER_WATCH_QUIET") == "1"),
         http: Keyword.get(opts, :http, &request/2)
       }}
    else
      _absent -> :ignore
    end
  end

  @impl true
  def fetch(state, since, limit) do
    url = "#{state.url}#{@path}?#{URI.encode_query(query(state, since, limit))}"

    with {:ok, body} <- state.http.(url, state.token),
         {:ok, payload} <- decode(body) do
      {:ok, items(payload), payload["hasMore"] == true, state}
    end
  end

  defp query(state, since, limit) do
    [
      dateAfter: DateTime.to_iso8601(since),
      sender: "others",
      limit: limit,
      excludeLowPriority: to_string(state.quiet?),
      includeMuted: to_string(not state.quiet?)
    ]
  end

  # The body carries message text, so it never reaches the error string: a
  # sweep failure is logged, and a log outlives the session.
  defp decode(body) do
    case JSON.decode(body) do
      {:ok, payload} when is_map(payload) -> {:ok, payload}
      _undecodable -> {:error, "undecodable response from Beeper Desktop"}
    end
  end

  # `chats` is a side-load: one entry per chat the page touched, carrying the
  # title and the network name a reader recognizes. Without it a line could
  # only name an account id.
  defp items(payload) do
    chats = Map.get(payload, "chats") || %{}

    payload
    |> Map.get("items")
    |> List.wrap()
    |> Enum.reject(&(&1["isSender"] == true))
    |> Enum.sort_by(&sent_at/1, DateTime)
    |> Enum.map(&item(&1, chats))
  end

  defp item(message, chats) do
    chat = Map.get(chats, message["chatID"]) || %{}

    %{
      id: to_string(message["id"]),
      platform: chat["network"],
      sender: message["senderName"],
      context: chat["title"],
      preview: text(message)
    }
  end

  # An attachment-only message has no text, and naming its kind beats an
  # empty line -- the kind is all the API gives without downloading the asset.
  defp text(message) do
    case message["text"] do
      text when is_binary(text) -> text
      _absent -> "[#{message["type"] || "attachment"}]"
    end
  end

  # A message whose timestamp will not parse still deserves announcing, so it
  # sorts to the front of the batch rather than vanishing from it.
  defp sent_at(message) do
    case DateTime.from_iso8601(to_string(message["timestamp"])) do
      {:ok, at, _offset} -> at
      _unparseable -> ~U[1970-01-01 00:00:00Z]
    end
  end

  defp token(opts) do
    case Keyword.fetch(opts, :token) do
      {:ok, token} -> {:ok, token}
      :error -> read_token(Path.expand(token_file()))
    end
  end

  defp token_file do
    System.get_env("IX_MCP_BEEPER_TOKEN_FILE") || @default_token_file
  end

  defp read_token(path) do
    with {:ok, contents} <- File.read(path),
         token when token != "" <- String.trim(contents) do
      {:ok, token}
    else
      _unusable -> :error
    end
  end

  defp request(url, token) do
    headers = [{~c"authorization", String.to_charlist("Bearer " <> token)}]

    case :httpc.request(
           :get,
           {String.to_charlist(url), headers},
           [{:timeout, @timeout_ms}],
           body_format: :binary
         ) do
      {:ok, {{_version, status, _reason}, _headers, body}} when status in 200..299 ->
        {:ok, body}

      {:ok, {{_version, status, _reason}, _headers, _body}} ->
        {:error, "Beeper Desktop returned HTTP #{status}"}

      {:error, reason} ->
        {:error, "Beeper Desktop unreachable: #{inspect(reason)}"}
    end
  end
end
