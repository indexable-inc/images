defmodule IxMcp.Inbox.Mail do
  @moduledoc """
  The mail feed: a new message in the signed-in mailbox becomes one channel
  line.

  Built on `IxMcp.Gmail`, and that choice is the whole point: the kernel
  already reaches the mailbox through the `:gmail_ex` NIF over the host's
  shared Google grant, tokens stay inside the NIF where neither a cell nor
  this watcher ever sees them, and so a mail feed costs no new credential,
  no new dependency, and nothing new to store. IMAP with its own password,
  or a second OAuth client, were the alternatives; both would have added a
  secret to keep.

  The watermark is Gmail's own `after:` term with epoch seconds, which the
  query syntax accepts and the client passes through. That is deliberate
  over reading the `date` off each hit: those are RFC 2822 in the sender's
  timezone, and nothing in this tree parses that today. The cost is that the
  boundary is the WATCHER's clock rather than the message's own date, which
  is the better end to be wrong on -- a mail that arrives carrying an old
  date, having sat in a relay, still announces on the sweep that first sees
  it.

  Each hit costs a metadata fetch inside the client, so the cadence here is
  a minute rather than the five seconds a loopback chat API can afford.

  Reads only.

  ## Configuration (none required)

    * absent, silently, unless the grant is present and covers the read
      scopes: `IxMcp.Gmail.status/0` decides, so a kernel that was never
      signed in never polls
    * `IX_MCP_MAIL_WATCH=0` turns the feed off
    * `IX_MCP_MAIL_WATCH_QUERY` overrides the default `in:inbox -from:me`
      (Gmail query syntax; the watermark is appended as `after:`)
    * `IX_MCP_MAIL_WATCH_INTERVAL_MS` overrides the 60s cadence
  """

  @behaviour IxMcp.Inbox.Source

  alias IxMcp.Inbox.Source

  # Arrival in the inbox is the event, so this is not `is:unread`: a mail
  # already read on a phone still matters to a session that has not heard of
  # it. `-from:me` drops the user's own sends, which land in the thread.
  @default_query "in:inbox -from:me"
  @default_interval_ms 60_000
  @detail_chars 200

  @impl true
  def label, do: "mail"

  @impl true
  def default_interval_ms do
    Source.interval_from_env("IX_MCP_MAIL_WATCH_INTERVAL_MS", @default_interval_ms)
  end

  @impl true
  def init(opts) do
    if System.get_env("IX_MCP_MAIL_WATCH") == "0" do
      :ignore
    else
      ready(Keyword.get(opts, :mail, IxMcp.Gmail), opts)
    end
  end

  # A signed-out, unconfigured, or under-scoped kernel is not an error to
  # report: this feed is meant to be on wherever it can be and absent
  # wherever it cannot. `status/0` is data by design and never raises, which
  # is what lets this be one match.
  defp ready(mail, opts) do
    case mail.status() do
      %{configured: true, signed_in: true, missing_scopes: []} ->
        {:ok, %{mail: mail, query: query(opts)}}

      _unavailable ->
        :ignore
    end
  end

  defp query(opts) do
    Keyword.get(opts, :query, System.get_env("IX_MCP_MAIL_WATCH_QUERY") || @default_query)
  end

  @impl true
  def fetch(state, since, limit) do
    query = "#{state.query} after:#{DateTime.to_unix(since)}"

    case state.mail.search(query, limit: limit) do
      {:ok, hits} ->
        # A full page is reported as overflow: the client returns the newest
        # `limit` hits and says nothing about a remainder, so "exactly full"
        # and "more than full" are the same observation from here.
        {:ok, items(hits), length(hits) >= limit, state}

      {:error, reason} ->
        {:error, detail(reason)}
    end
  end

  # Newest first from the API, oldest first on the channel.
  defp items(hits) do
    hits |> Enum.reverse() |> Enum.map(&item/1)
  end

  defp item(hit) do
    %{
      id: to_string(Map.get(hit, :id)),
      platform: "email",
      sender: Map.get(hit, :from),
      context: Map.get(hit, :subject),
      preview: Map.get(hit, :snippet)
    }
  end

  # The error is a client struct about the transport (auth, quota, network),
  # never about a message body, but it is still clipped: it goes to a log.
  defp detail(%{message: message}) when is_binary(message) do
    String.slice(message, 0, @detail_chars)
  end

  defp detail(reason) when is_binary(reason), do: String.slice(reason, 0, @detail_chars)
  defp detail(reason), do: String.slice(inspect(reason), 0, @detail_chars)
end
