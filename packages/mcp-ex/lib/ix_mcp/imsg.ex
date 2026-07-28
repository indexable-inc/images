defmodule IxMcp.Imsg do
  @moduledoc """
  iMessage from a cell -- send, browse, and search the signed-in user's
  Messages. `Imsg` in the workspace prelude.

      Imsg.send("+14155551212", "on my way")
      Imsg.chats(limit: 10)
      Imsg.recent(with: "+14155551212")
      Imsg.search("lunch")

  Reads `~/Library/Messages/chat.db` read-only (needs Full Disk Access,
  which the kernel has) and sends through Messages.app via osascript, so
  everything here works only on the mac the kernel runs on; elsewhere the
  calls return `{:error, _}`.

  A message's `text` column is often NULL -- the user's own sends in
  particular -- with the real body inside the `attributedBody`
  typedstream. `decode_body/1` extracts it, and every helper here returns
  the decoded text already; a send therefore cannot be found again by
  `WHERE text LIKE`, only through `search/2`.

  Handles are phone numbers in +E.164 form or iMessage email addresses;
  `Contacts.search/2` maps names to handles.
  """

  alias IxMcp.Sqlite

  @osascript "/usr/bin/osascript"

  @doc """
  Send `text` over iMessage to the handle `to`. Returns as soon as
  Messages.app accepts the message; delivery is asynchronous, so when it
  matters check the message's `error` flag via `recent/1` a moment later.
  """
  @spec send(String.t(), String.t()) :: :ok | {:error, String.t()}
  def send(to, text) do
    script = """
    tell application "Messages"
      set svc to 1st account whose service type = iMessage
      send #{applescript_str(text)} to participant #{applescript_str(to)} of svc
    end tell
    """

    if File.exists?(@osascript) do
      case System.cmd(@osascript, ["-e", script], stderr_to_stdout: true) do
        {_, 0} -> :ok
        {out, code} -> {:error, "osascript exit #{code}: #{String.trim(out)}"}
      end
    else
      {:error, "#{@osascript} not found: sending needs the macOS host"}
    end
  end

  @doc """
  Recent chats, most recently active first: `guid` (the stable id other
  helpers take via `chat:`), `identifier` (the handle, or an opaque hex
  id for group chats), `name` (group display name, often empty),
  `last_at`, and message count `n`. Options: `limit:` (default 20),
  `db:`.
  """
  @spec chats(keyword()) :: {:ok, [map()]} | {:error, String.t()}
  def chats(opts \\ []) do
    Sqlite.query(
      db(opts),
      """
      SELECT c.guid, c.chat_identifier AS identifier, c.display_name AS name,
        datetime(MAX(m.date)/1000000000 + 978307200, 'unixepoch', 'localtime') AS last_at,
        COUNT(m.ROWID) AS n
      FROM chat c
      JOIN chat_message_join cmj ON cmj.chat_id = c.ROWID
      JOIN message m ON m.ROWID = cmj.message_id
      GROUP BY c.ROWID
      ORDER BY MAX(m.date) DESC
      LIMIT ?
      """,
      [limit(opts)]
    )
  end

  @doc """
  Recent messages, newest first: sender handle (`"me"` for own sends),
  decoded text, chat guid, timestamp, and the async-delivery `error`
  flag. Options: `with:` (a handle; selects every chat it participates
  in, groups included -- group chats have opaque identifiers, so go
  through handles, never chat names), `chat:` (a guid from `chats/1`),
  `limit:` (default 20), `db:`.
  """
  @spec recent(keyword()) :: {:ok, [map()]} | {:error, String.t()}
  def recent(opts \\ []) do
    filters =
      Enum.reject(
        [
          opts[:chat] && {"c.guid = ?", opts[:chat]},
          opts[:with] &&
            {"""
             c.ROWID IN (SELECT chj.chat_id FROM chat_handle_join chj
               JOIN handle h2 ON h2.ROWID = chj.handle_id
               WHERE h2.id = ?)
             """, opts[:with]}
        ],
        &is_nil/1
      )

    messages(opts, Enum.map(filters, &elem(&1, 0)), Enum.map(filters, &elem(&1, 1)))
  end

  @doc """
  Search message text, newest first, same row shape as `recent/1`.
  Plain-text rows match case-insensitively; rich-text rows (NULL `text`,
  body in the typedstream) are prefiltered byte-wise, so they match the
  query as typed plus its lower, UPPER, and Capitalized variants.
  Options: `limit:` (default 20), `db:`.
  """
  @spec search(String.t(), keyword()) :: {:ok, [map()]} | {:error, String.t()}
  def search(query, opts \\ []) do
    variants =
      [query, String.downcase(query), String.upcase(query), String.capitalize(query)]
      |> Enum.uniq()

    clauses =
      ["m.text LIKE ?" | List.duplicate("instr(hex(m.attributedBody), ?) > 0", length(variants))]

    params = ["%" <> query <> "%" | Enum.map(variants, &Base.encode16/1)]
    messages(opts, ["(" <> Enum.join(clauses, " OR ") <> ")"], params)
  end

  @doc """
  The text inside a message's raw `attributedBody` typedstream blob; nil
  when there is none to find.
  """
  @spec decode_body(binary() | nil) :: String.t() | nil
  def decode_body(nil), do: nil

  def decode_body(bin) do
    with {i, _} <- :binary.match(bin, "NSString"),
         rest = binary_part(bin, i, byte_size(bin) - i),
         {j, _} <- :binary.match(rest, "+"),
         text when is_binary(text) <- payload(binary_part(rest, j + 1, byte_size(rest) - j - 1)),
         true <- String.valid?(text) do
      text
    else
      _ -> nil
    end
  end

  # Typedstream length prefix: one byte below 0x80, or 0x81 then u16le.
  defp payload(<<0x81, len::little-16, text::binary-size(len), _::binary>>), do: text
  defp payload(<<len::8, text::binary-size(len), _::binary>>) when len < 0x80, do: text
  defp payload(_), do: nil

  defp messages(opts, filters, params) do
    where = if filters == [], do: "", else: "WHERE " <> Enum.join(filters, " AND ")

    rows =
      Sqlite.query(
        db(opts),
        """
        SELECT m.ROWID AS id, c.guid AS chat, h.id AS sender, m.is_from_me,
          m.error, m.text, m.attributedBody AS body,
          datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') AS at
        FROM message m
        JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
        JOIN chat c ON c.ROWID = cmj.chat_id
        LEFT JOIN handle h ON h.ROWID = m.handle_id
        #{where}
        ORDER BY m.date DESC
        LIMIT ?
        """,
        params ++ [limit(opts)]
      )

    with {:ok, rows} <- rows do
      {:ok,
       Enum.map(rows, fn row ->
         %{
           id: row["id"],
           chat: row["chat"],
           sender: if(row["is_from_me"] == 1, do: "me", else: row["sender"]),
           text: row["text"] || decode_body(row["body"]),
           at: row["at"],
           error: row["error"]
         }
       end)}
    end
  end

  defp db(opts), do: opts[:db] || Path.expand("~/Library/Messages/chat.db")

  defp limit(opts), do: Keyword.get(opts, :limit, 20)

  defp applescript_str(s) do
    ~s{"} <> (s |> String.replace("\\", "\\\\") |> String.replace(~s{"}, ~s{\\"})) <> ~s{"}
  end
end
