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

  alias IxMcp.Mac
  alias IxMcp.Sqlite

  @osascript "/usr/bin/osascript"

  # RTF control words for the formats Messages exposes in its Format menu.
  # Each maps to a __kIMText*AttributeName the receiving client honours.
  @formats [italic: "\\i", bold: "\\b", underline: "\\ul", strike: "\\strike"]

  @doc """
  Send `text` over iMessage to the handle `to`. Returns as soon as
  Messages.app accepts the message; delivery is asynchronous, so when it
  matters check the message's `error` flag via `recent/1` a moment later.

  `mac:` picks which machine sends: `Mac.local()` (the default, the mac the
  kernel runs on, signed in as the user) or `Mac.guest(node)` for a macOS
  guest VM signed in as the agent's own Apple ID.

  Formatting options -- `italic:`, `bold:`, `underline:`, `strike:` --
  apply to the whole message and set the same rich-text attributes
  (`__kIMTextItalicAttributeName` and friends) that Messages' own Format
  menu does, so they render as real formatting rather than as the
  unicode-lookalike codepoints that survive a plain send.

  A formatted send costs far more than a plain one. AppleScript's `send`
  takes a string and drops every attribute, and the only other route to
  those attributes is IMCore, a private framework only a signed helper
  could link. So formatting drives the UI instead: it brings Messages to
  the front, puts an RTF flavor on the clipboard, pastes, presses return,
  and puts the clipboard back. That steals focus for a second or two and
  can misfire if the user is typing into another window, so pass a format
  flag only where the formatting itself is the point.
  """
  @spec send(String.t(), String.t(), keyword()) :: :ok | {:error, String.t()}
  def send(to, text, opts \\ []) do
    mac = Mac.from_opts(opts)

    with :ok <- osascript_present(mac),
         :ok <- deliver(mac, to, text, opts) do
      notify(mac, to, text, opts)
    end
  end

  defp deliver(mac, to, text, opts) do
    case Enum.filter(@formats, fn {opt, _} -> opts[opt] end) do
      [] -> osascript(mac, plain_script(to, text))
      formats -> formatted(mac, to, text, formats)
    end
  end

  defp formatted(mac, to, text, formats) do
    if Mac.local?(mac),
      do: send_formatted(to, text, Enum.map(formats, &elem(&1, 1))),
      else: {:error, formatting_unsupported(mac)}
  end

  # A formatted send reads the message back out of chat.db to prove the
  # keystrokes landed, and that read runs through the exqlite NIF, which a
  # guest node running bare Elixir does not carry. Refusing is better than
  # sending unconfirmed: the UI path is the one that can silently type into
  # the wrong window. Lifting this means shipping the full kernel release to
  # the guest instead of a bare node.
  defp formatting_unsupported(mac) do
    "formatted sends run only on this mac, not on #{Mac.describe(mac)}: " <>
      "confirming one needs a chat.db read the guest node cannot do"
  end

  # Messages posts no banner for a message this machine sent, so an agent
  # texting as the user is invisible until the user opens the thread. This
  # is the only signal that it happened; pass `notify: false` where a burst
  # of sends would make it noise. A banner that fails never fails the send,
  # because by then the message is already gone.
  defp notify(mac, to, text, opts) do
    if Keyword.get(opts, :notify, true) do
      osascript(
        mac,
        "display notification #{applescript_str(text)} " <>
          "with title #{applescript_str("Agent sent an iMessage")} " <>
          "subtitle #{applescript_str(to)} sound name \"Glass\""
      )
    end

    :ok
  end

  defp plain_script(to, text) do
    """
    tell application "Messages"
      set svc to 1st account whose service type = iMessage
      send #{applescript_str(text)} to participant #{applescript_str(to)} of svc
    end tell
    """
  end

  defp send_formatted(to, text, controls) do
    path = Path.join(System.tmp_dir!(), "ix-imsg-#{System.unique_integer([:positive])}.rtf")

    try do
      File.write!(path, rtf(text, controls))

      with :ok <- osascript(Mac.local(), paste_script(to, path)), do: confirm_sent(to, text)
    after
      File.rm(path)
    end
  end

  # `open location` selects the conversation, so this addresses handles
  # only; a group chat's guid is not a URL Messages will open.
  defp paste_script(to, rtf_path) do
    """
    set saved to the clipboard as record
    set the clipboard to (read (POSIX file #{applescript_str(rtf_path)}) as «class RTF »)
    tell application "Messages" to activate
    open location #{applescript_str("imessage://" <> to)}
    tell application "System Events" to tell process "Messages"
      repeat 50 times
        if exists window 1 then exit repeat
        delay 0.1
      end repeat
      delay 0.6
      keystroke "v" using command down
      delay 0.4
      key code 36
      delay 0.4
    end tell
    set the clipboard to saved
    """
  end

  # A UI-driven send lands wherever the keystrokes land, and osascript
  # reports success either way, so read the message back out of chat.db
  # rather than trusting the exit code. Messages writes the row a beat
  # after the return key; 10 polls at 300ms covered every observed lag
  # here, and a miss reports failure instead of silently dropping a
  # message the caller believes it sent.
  defp confirm_sent(to, text, tries \\ 10) do
    case recent(with: to, limit: 1) do
      {:ok, [%{sender: "me", text: ^text} | _]} ->
        :ok

      {:ok, _} when tries > 1 ->
        Process.sleep(300)
        confirm_sent(to, text, tries - 1)

      {:ok, _} ->
        {:error, "Messages took the paste but no matching message reached chat.db"}

      {:error, reason} ->
        {:error, reason}
    end
  end

  # \\fs28 is 14pt, matching the compose field, so a paste does not arrive
  # in a different size from what the user types.
  defp rtf(text, controls) do
    on = Enum.join(controls, " ")
    off = Enum.map_join(controls, " ", &(&1 <> "0"))

    "{\\rtf1\\ansi\\ansicpg1252\n{\\fonttbl\\f0\\fnil Helvetica;}\n" <>
      "\\f0\\fs28 #{on} #{rtf_escape(text)}#{off}}"
  end

  # RTF is 7-bit and reserves braces and backslash, so anything else has
  # to leave as a \\uN escape or it arrives as mojibake. N is signed
  # 16-bit and the trailing `?` is the mandatory fallback glyph, so an
  # astral codepoint (emoji) goes out as its surrogate pair.
  defp rtf_escape(text) do
    text
    |> String.to_charlist()
    |> Enum.map_join(fn
      ?\\ -> "\\\\"
      ?{ -> "\\{"
      ?} -> "\\}"
      ?\n -> "\\line "
      c when c < 128 -> <<c>>
      c when c < 0x10000 -> "\\u#{signed16(c)}?"
      c -> surrogates(c)
    end)
  end

  defp surrogates(c) do
    offset = c - 0x10000
    hi = 0xD800 + Bitwise.bsr(offset, 10)
    lo = 0xDC00 + Bitwise.band(offset, 0x3FF)
    "\\u#{signed16(hi)}?\\u#{signed16(lo)}?"
  end

  defp signed16(c) when c > 0x7FFF, do: c - 0x10000
  defp signed16(c), do: c

  defp osascript_present(mac) do
    if Mac.exists?(mac, @osascript),
      do: :ok,
      else: {:error, "#{@osascript} not found on #{Mac.describe(mac)}: sending needs macOS"}
  end

  defp osascript(mac, script) do
    with {:ok, _} <- Mac.cmd(mac, @osascript, ["-e", script]), do: :ok
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
