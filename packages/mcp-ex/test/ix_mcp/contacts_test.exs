defmodule IxMcp.ContactsTest do
  use ExUnit.Case, async: true

  alias IxMcp.Contacts

  @tag :tmp_dir
  test "search folds phone/email join fan-out into one map per person", %{tmp_dir: dir} do
    db =
      seed(Path.join(dir, "AddressBook-v22.abcddb"), [
        "CREATE TABLE ZABCDRECORD (Z_PK INTEGER PRIMARY KEY, ZFIRSTNAME TEXT, ZLASTNAME TEXT, ZORGANIZATION TEXT)",
        "CREATE TABLE ZABCDPHONENUMBER (Z_PK INTEGER PRIMARY KEY, ZOWNER INTEGER, ZFULLNUMBER TEXT)",
        "CREATE TABLE ZABCDEMAILADDRESS (Z_PK INTEGER PRIMARY KEY, ZOWNER INTEGER, ZADDRESS TEXT)",
        "INSERT INTO ZABCDRECORD VALUES (1, 'Ada', 'Lovelace', 'Analytical Engines')",
        "INSERT INTO ZABCDRECORD VALUES (2, 'Adam', NULL, NULL)",
        "INSERT INTO ZABCDPHONENUMBER VALUES (1, 1, '+15550002222'), (2, 1, '+15550003333')",
        "INSERT INTO ZABCDEMAILADDRESS VALUES (1, 1, 'ada@example.com')"
      ])

    {:ok, [ada]} = Contacts.search("lovelace", dbs: [db])
    assert ada.name == "Ada Lovelace"
    assert Enum.sort(ada.phones) == ["+15550002222", "+15550003333"]
    assert ada.emails == ["ada@example.com"]

    {:ok, both} = Contacts.search("ada", dbs: [db])
    assert Enum.map(both, & &1.name) == ["Ada Lovelace", "Adam"]

    assert {:ok, []} = Contacts.search("nobody", dbs: [db])
  end

  test "no databases at all is an error" do
    assert {:error, msg} = Contacts.search("x", dbs: [])
    assert msg =~ "no AddressBook database"
  end

  defp seed(db, statements) do
    {:ok, conn} = Exqlite.Sqlite3.open(db)
    Enum.each(statements, fn sql -> :ok = Exqlite.Sqlite3.execute(conn, sql) end)
    :ok = Exqlite.Sqlite3.close(conn)
    db
  end
end
