defmodule IxMcp.ImsgTest do
  use ExUnit.Case, async: true

  alias IxMcp.Imsg

  # A minimal typedstream: the decoder anchors on "NSString", skips to
  # the "+" marker, and reads a length-prefixed payload.
  defp body(text) do
    "stream" <> "NSString" <> <<1, 0x94, 0x84>> <> "+" <> len(text) <> text <> <<0x86>>
  end

  defp len(text) when byte_size(text) < 0x80, do: <<byte_size(text)>>
  defp len(text), do: <<0x81, byte_size(text)::little-16>>

  test "decodes a short-form body" do
    assert Imsg.decode_body(body("hello there")) == "hello there"
  end

  test "decodes a long-form (0x81 u16le) body" do
    text = String.duplicate("na", 200)
    assert Imsg.decode_body(body(text)) == text
  end

  test "nil blob and non-text blobs decode to nil" do
    assert Imsg.decode_body(nil) == nil
    assert Imsg.decode_body("no marker here") == nil
    assert Imsg.decode_body("NSString without plus") == nil
  end

  @tag :tmp_dir
  test "recent and search read a chat.db, decoding NULL-text rows", %{tmp_dir: dir} do
    db = seed_chat_db(dir)

    {:ok, [reply, hello]} = Imsg.recent(db: db, with: "+15550001111")
    assert %{sender: "+15550001111", text: "hello from alice", error: 0} = hello
    # Own send: NULL text, body only in the typedstream.
    assert %{sender: "me", text: "typedstream reply"} = reply

    {:ok, hits} = Imsg.search("Typedstream", db: db)
    assert [%{text: "typedstream reply"}] = hits

    {:ok, [chat]} = Imsg.chats(db: db)
    assert %{"guid" => "any;-;+15550001111", "n" => 2} = chat
  end

  test "missing database is an error, not a crash" do
    assert {:error, msg} = Imsg.recent(db: "/nonexistent/chat.db")
    assert msg =~ "no such database"
  end

  defp seed_chat_db(dir) do
    db = Path.join(dir, "chat.db")

    seed(db, [
      "CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, guid TEXT, chat_identifier TEXT, display_name TEXT)",
      "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT)",
      "CREATE TABLE message (ROWID INTEGER PRIMARY KEY, handle_id INTEGER, text TEXT, attributedBody BLOB, date INTEGER, is_from_me INTEGER, error INTEGER DEFAULT 0)",
      "CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER)",
      "CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER)",
      "INSERT INTO chat VALUES (1, 'any;-;+15550001111', '+15550001111', '')",
      "INSERT INTO handle VALUES (1, '+15550001111')",
      "INSERT INTO chat_handle_join VALUES (1, 1)",
      "INSERT INTO message VALUES (1, 1, 'hello from alice', NULL, 1000000000, 0, 0)",
      "INSERT INTO message VALUES (2, 1, NULL, X'#{Base.encode16(body("typedstream reply"))}', 2000000000, 1, 0)",
      "INSERT INTO chat_message_join VALUES (1, 1), (1, 2)"
    ])
  end

  defp seed(db, statements) do
    {:ok, conn} = Exqlite.Sqlite3.open(db)
    Enum.each(statements, fn sql -> :ok = Exqlite.Sqlite3.execute(conn, sql) end)
    :ok = Exqlite.Sqlite3.close(conn)
    db
  end
end
