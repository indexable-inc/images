{
  bundledSource,
  bundledTestPython,
  importTest,
  lib,
  pkgs,
}: let
  # Native macOS iMessage access, bundled like `screen`/`vmkit` so every session
  # can `import imessage` on Darwin. Pure Python over the bundled sqlite3/polars
  # (plus Foundation's NSUnarchiver to decode the archived message text): it reads
  # the Messages and Contacts SQLite databases into polars frames, sends new
  # messages through the Messages app over AppleScript, and edits contacts
  # through the Contacts app over JXA (so edits sync to iCloud). macOS-only; the
  # module raises off Darwin.
  imessagePythonSource = bundledSource {
    name = "ix-mcp-imessage-python-source";
    path = ./.;
  };
  imessageModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-imessage-python-module"
    {
      strictDeps = true;
      meta.description = "Native macOS iMessage read-to-polars + send bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/imessage"
      mkdir -p "$site"
      cp -r ${imessagePythonSource}/imessage/. "$site/"
    ''
  );
  imessageBundled = importTest [imessageModule] "imessage" "import imessage; print('imessage-ok', all(callable(getattr(imessage, n)) for n in ('messages', 'chats', 'contacts', 'send')))";
  # The imessage module: read the Messages/Contacts SQLite databases into polars
  # and (without sending) validate the AppleScript send path. Hermetic -- it
  # builds tiny fixture databases with the real schema subset and round-trips an
  # archived NSAttributedString through Foundation, so the sandbox runs it.
  imessageTestPy = pkgs.writeText "ix-mcp-imessage-test.py" ''
    # python

    import os
    import sqlite3
    import tempfile
    from datetime import datetime, timezone

    import polars as pl

    import imessage

    assert imessage.CHAT_DB.endswith("Library/Messages/chat.db"), imessage.CHAT_DB

    # attributedBody round-trip: archive an NSAttributedString the way macOS does,
    # then assert our NSUnarchiver-based decoder recovers the exact text (incl.
    # non-ASCII), and that junk/None decode to None rather than raising.
    import Foundation

    s = Foundation.NSAttributedString.alloc().initWithString_("héllo 👋 world")
    blob = bytes(Foundation.NSArchiver.archivedDataWithRootObject_(s))
    assert imessage._decode_attributed_body(blob) == "héllo 👋 world"
    assert imessage._decode_attributed_body(b"not an archive") is None
    assert imessage._decode_attributed_body(None) is None

    # normalization lines up handles with address-book entries.
    assert imessage._norm("+1 (202) 555-0123") == imessage._norm("2025550123")
    assert imessage._norm("Me@Example.COM ") == "me@example.com"

    # reply/tapback reference parsing: strip the "p:<part>/" or "bp:" prefix to the
    # bare GUID, and decode associated_message_type into a tapback label.
    assert imessage._bare_guid("p:0/ABC") == "ABC"
    assert imessage._bare_guid("bp:XYZ") == "XYZ"
    assert imessage._bare_guid("ABC") == "ABC"
    assert imessage._bare_guid(None) is None
    assert imessage._tapback(2000) == "loved"
    assert imessage._tapback(2005) == "questioned"
    assert imessage._tapback(3003) == "removed-laughed"
    # base outside 2xxx/3xxx is not a tapback even when the low digit collides
    # with a reaction offset (1000 is an inline association, not a "loved").
    assert imessage._tapback(1000) is None
    assert imessage._tapback(2) is None
    assert imessage._tapback(0) is None
    assert imessage._tapback(None) is None

    work = tempfile.mkdtemp()
    chat = os.path.join(work, "chat.db")
    con = sqlite3.connect(chat)
    con.executescript(
        """
        CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
        CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, display_name TEXT, chat_identifier TEXT, service_name TEXT);
        CREATE TABLE message (ROWID INTEGER PRIMARY KEY, guid TEXT, date INTEGER, text TEXT, attributedBody BLOB, is_from_me INTEGER, is_read INTEGER, service TEXT, handle_id INTEGER, thread_originator_guid TEXT, associated_message_guid TEXT, associated_message_type INTEGER DEFAULT 0, date_edited INTEGER DEFAULT 0, date_retracted INTEGER DEFAULT 0);
        CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
        CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
        """
    )
    apple_epoch = datetime(2001, 1, 1, tzinfo=timezone.utc)

    def ns(dt):
        return int((dt - apple_epoch).total_seconds() * 1000000000)

    t1 = datetime(2024, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
    t2 = datetime(2024, 1, 2, 3, 5, 5, tzinfo=timezone.utc)
    con.execute("INSERT INTO handle VALUES (1, '+12025550123')")
    con.execute("INSERT INTO chat VALUES (1, NULL, '+12025550123', 'iMessage')")
    con.execute("INSERT INTO message (ROWID, guid, date, text, attributedBody, is_from_me, is_read, service, handle_id) VALUES (1, 'm1', ?, 'plain text', NULL, 1, 1, 'iMessage', 1)", (ns(t1),))
    con.execute("INSERT INTO message (ROWID, guid, date, text, attributedBody, is_from_me, is_read, service, handle_id) VALUES (2, 'm2', ?, NULL, ?, 0, 0, 'iMessage', 1)", (ns(t2), blob))
    con.execute("INSERT INTO chat_message_join VALUES (1, 1), (1, 2)")
    con.execute("INSERT INTO message_attachment_join VALUES (2, 99)")
    con.commit()
    con.close()

    # resolve_names defaults True; with no address book under $HOME it must degrade
    # to name=None rather than fail.
    df = imessage.messages(db=chat, limit=10)
    assert df.height == 2, df
    assert df.schema["date"] == pl.Datetime("ns", "UTC"), df.schema
    # newest first; attributedBody is decoded into `text`.
    assert df["rowid"].to_list() == [2, 1], df
    assert df["text"].to_list() == ["héllo 👋 world", "plain text"], df["text"].to_list()
    assert df["is_from_me"].to_list() == [False, True], df
    assert df["name"].to_list() == [None, None], df
    # the relational columns default cleanly when a message is none of these.
    assert df["reply_to_guid"].to_list() == [None, None], df
    assert df["reply_to_rowid"].to_list() == [None, None], df
    assert df["tapback"].to_list() == [None, None], df
    assert df["edited"].to_list() == [False, False], df
    assert df["unsent"].to_list() == [False, False], df

    row2 = df.filter(pl.col("rowid") == 2)
    assert row2["date"][0] == t2, (row2["date"][0], t2)
    assert row2["n_attachments"][0] == 1, row2

    # filters: from_me, contact (by handle, normalized), since, and a no-match.
    assert imessage.messages(db=chat, from_me=True).height == 1
    assert imessage.messages(db=chat, contact="(202) 555-0123").height == 2
    assert imessage.messages(db=chat, contact="+19999999999").height == 0
    since = imessage.messages(db=chat, since=t2)
    assert since.height == 1 and since["rowid"][0] == 2, since
    # a no-match still returns the full, typed (datetime) schema.
    assert imessage.messages(db=chat, contact="+19999999999").schema["date"] == pl.Datetime("ns", "UTC")

    chats = imessage.chats(db=chat)
    assert chats.height == 1 and chats["n_messages"][0] == 2, chats
    assert chats["chat_identifier"][0] == "+12025550123", chats
    assert chats.schema["last_date"] == pl.Datetime("ns", "UTC"), chats.schema

    # Reads must see un-checkpointed WAL rows: chat.db runs in WAL mode and a
    # message you just sent sits in the -wal until a checkpoint. A writer keeps a
    # WAL connection open (no checkpoint) after inserting; messages() must still
    # see the new row -- it would not under immutable=1. Guards send-then-read-back.
    writer = sqlite3.connect(chat)
    writer.execute("PRAGMA journal_mode=WAL")
    writer.execute(
        "INSERT INTO message (ROWID, guid, date, text, attributedBody, is_from_me, is_read, service, handle_id) VALUES (3, 'm3', ?, 'in the wal', NULL, 1, 1, 'iMessage', 1)",
        (ns(datetime(2024, 1, 2, 3, 6, 5, tzinfo=timezone.utc)),),
    )
    writer.execute("INSERT INTO chat_message_join VALUES (1, 3)")
    writer.commit()
    assert "in the wal" in imessage.messages(db=chat)["text"].to_list(), "WAL rows must be visible"
    writer.close()

    # reply threading, tapbacks, edits, and unsends: insert one of each and assert
    # messages() resolves a threaded reply to its originator and decodes the rest.
    con = sqlite3.connect(chat)
    cols2 = "(ROWID, guid, date, text, attributedBody, is_from_me, is_read, service, handle_id, thread_originator_guid, associated_message_guid, associated_message_type, date_edited, date_retracted)"
    t3 = datetime(2024, 1, 2, 4, 0, 0, tzinfo=timezone.utc)
    # a threaded reply to message 1 ("plain text"); the originator ref carries a
    # "p:0/" part prefix that must be stripped back to the bare guid "m1".
    con.execute("INSERT INTO message " + cols2 + " VALUES (10, 'm10', ?, 'a reply', NULL, 0, 1, 'iMessage', 1, 'p:0/m1', NULL, 0, 0, 0)", (ns(t3),))
    # a Loved tapback on message 2, and a removed-liked on message 1.
    con.execute("INSERT INTO message " + cols2 + " VALUES (11, 'm11', ?, 'Loved a message', NULL, 1, 1, 'iMessage', 1, NULL, 'p:0/m2', 2000, 0, 0)", (ns(t3),))
    con.execute("INSERT INTO message " + cols2 + " VALUES (12, 'm12', ?, 'Removed a like', NULL, 1, 1, 'iMessage', 1, NULL, 'p:0/m1', 3001, 0, 0)", (ns(t3),))
    # an edited message and an unsent (retracted) message.
    con.execute("INSERT INTO message " + cols2 + " VALUES (13, 'm13', ?, 'fixed typo', NULL, 1, 1, 'iMessage', 1, NULL, NULL, 0, ?, 0)", (ns(t3), ns(t3)))
    con.execute("INSERT INTO message " + cols2 + " VALUES (14, 'm14', ?, 'oops', NULL, 1, 1, 'iMessage', 1, NULL, NULL, 0, 0, ?)", (ns(t3), ns(t3)))
    # associated_message_type 1000 is a non-tapback association: it must not be
    # mislabeled "loved" just because its low digit is the loved offset.
    con.execute("INSERT INTO message " + cols2 + " VALUES (15, 'm15', ?, 'inline assoc', NULL, 1, 1, 'iMessage', 1, NULL, 'p:0/m1', 1000, 0, 0)", (ns(t3),))
    con.execute("INSERT INTO chat_message_join VALUES (1, 10), (1, 11), (1, 12), (1, 13), (1, 14), (1, 15)")
    con.commit()
    con.close()

    thr = imessage.messages(db=chat, limit=50)
    reply = thr.filter(pl.col("rowid") == 10).row(0, named=True)
    assert reply["reply_to_guid"] == "m1", reply
    assert reply["reply_to_rowid"] == 1, reply
    assert reply["reply_to_text"] == "plain text", reply
    love = thr.filter(pl.col("rowid") == 11).row(0, named=True)
    assert love["tapback"] == "loved" and love["tapback_target_guid"] == "m2", love
    removed = thr.filter(pl.col("rowid") == 12).row(0, named=True)
    assert removed["tapback"] == "removed-liked" and removed["tapback_target_guid"] == "m1", removed
    assert thr.filter(pl.col("rowid") == 13)["edited"][0] is True
    assert thr.filter(pl.col("rowid") == 14)["unsent"][0] is True
    assert thr.filter(pl.col("rowid") == 15)["tapback"][0] is None, thr.filter(pl.col("rowid") == 15)
    # a plain message carries none of this metadata.
    plain = thr.filter(pl.col("rowid") == 1).row(0, named=True)
    assert plain["reply_to_guid"] is None and plain["tapback"] is None, plain
    assert plain["edited"] is False and plain["unsent"] is False, plain

    # legacy chat.db compatibility: a pre-macOS-13 schema has no
    # thread_originator_guid / date_edited / date_retracted columns. messages()
    # must select those only when present and degrade to empty fields, not raise.
    legacy = os.path.join(work, "legacy.db")
    con = sqlite3.connect(legacy)
    con.executescript(
        """
        CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
        CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, display_name TEXT, chat_identifier TEXT, service_name TEXT);
        CREATE TABLE message (ROWID INTEGER PRIMARY KEY, guid TEXT, date INTEGER, text TEXT, attributedBody BLOB, is_from_me INTEGER, is_read INTEGER, service TEXT, handle_id INTEGER);
        CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
        CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
        """
    )
    con.execute("INSERT INTO handle VALUES (1, '+12025550123')")
    con.execute("INSERT INTO message (ROWID, guid, date, text, is_from_me, is_read, service, handle_id) VALUES (1, 'g1', ?, 'legacy', 1, 1, 'iMessage', 1)", (ns(t1),))
    con.execute("INSERT INTO chat_message_join VALUES (1, 1)")
    con.commit()
    con.close()
    leg = imessage.messages(db=legacy, limit=5)
    assert leg.height == 1 and leg["text"][0] == "legacy", leg
    assert leg["reply_to_guid"][0] is None and leg["tapback"][0] is None, leg
    assert leg["edited"][0] is False and leg["unsent"][0] is False, leg

    # contacts: phones/emails aggregate into list columns under one display name.
    ab = os.path.join(work, "ab.abcddb")
    con = sqlite3.connect(ab)
    con.executescript(
        """
        CREATE TABLE ZABCDRECORD (Z_PK INTEGER PRIMARY KEY, ZFIRSTNAME TEXT, ZLASTNAME TEXT, ZORGANIZATION TEXT, ZNICKNAME TEXT);
        CREATE TABLE ZABCDPHONENUMBER (Z_PK INTEGER PRIMARY KEY, ZOWNER INTEGER, ZFULLNUMBER TEXT);
        CREATE TABLE ZABCDEMAILADDRESS (Z_PK INTEGER PRIMARY KEY, ZOWNER INTEGER, ZADDRESS TEXT);
        """
    )
    con.execute("INSERT INTO ZABCDRECORD VALUES (1, 'Ada', 'Lovelace', NULL, NULL)")
    con.execute("INSERT INTO ZABCDPHONENUMBER VALUES (1, 1, '+1 (202) 555-0123')")
    con.execute("INSERT INTO ZABCDPHONENUMBER VALUES (2, 1, '+12025550124')")
    con.execute("INSERT INTO ZABCDEMAILADDRESS VALUES (1, 1, 'ada@x.com')")
    con.commit()
    con.close()
    co = imessage.contacts(db=ab)
    assert co.height == 1, co
    rec = co.row(0, named=True)
    assert rec["name"] == "Ada Lovelace", rec
    assert set(rec["phones"]) == {"+1 (202) 555-0123", "+12025550124"}, rec
    assert rec["emails"] == ["ada@x.com"], rec

    # send: rejects an unknown service and never sends here; the script carries the
    # service token and takes recipient/body as run-arguments (no injection).
    try:
        imessage.send("+12025550123", "nope", service="bogus")
    except ValueError:
        pass
    else:
        raise SystemExit("send must reject an unknown service")
    assert "service type = iMessage" in imessage._SEND_SCRIPT.format(service="iMessage")

    print("imessage-ok")
  '';
  imessageTestPython = bundledTestPython [imessageModule];
  imessageSmoke =
    pkgs.runCommand "ix-mcp-imessage-smoke"
    {
      nativeBuildInputs = [imessageTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe imessageTestPython} ${imessageTestPy} >stdout 2>stderr || {
        echo "ix-mcp imessage smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'imessage-ok' stdout || {
        echo "ix-mcp imessage smoke did not confirm the imessage module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = imessageModule;
  darwinOnly = true;
  tests = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
    inherit
      imessageBundled
      imessageSmoke
      ;
  };
}
