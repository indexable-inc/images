defmodule IxMcp.Contacts do
  @moduledoc """
  The user's macOS address book from a cell, `Contacts` in the workspace
  prelude: name -> phone numbers and emails, the handles `Imsg` takes.

      {:ok, [%{name: "Hari", phones: ["+14343101227"], ...}]} =
        Contacts.search("hari")

  Reads every `AddressBook-v22.abcddb` under
  `~/Library/Application Support/AddressBook` read-only -- the populated
  database usually lives under `Sources/<uuid>/`, not at the top level,
  so all of them are unioned.
  """

  alias IxMcp.Sqlite

  @doc """
  Contacts whose first name, last name, full name, or organization
  contains `name` (case-insensitive): `name`, `org`, `phones`, `emails`.
  Options: `limit:` (default 20), `dbs:` (paths, for tests).
  """
  @spec search(String.t(), keyword()) :: {:ok, [map()]} | {:error, String.t()}
  def search(name, opts \\ []) do
    sql = """
    SELECT r.Z_PK AS pk, r.ZFIRSTNAME AS first, r.ZLASTNAME AS last,
      r.ZORGANIZATION AS org, p.ZFULLNUMBER AS phone, e.ZADDRESS AS email
    FROM ZABCDRECORD r
    LEFT JOIN ZABCDPHONENUMBER p ON p.ZOWNER = r.Z_PK
    LEFT JOIN ZABCDEMAILADDRESS e ON e.ZOWNER = r.Z_PK
    WHERE r.ZFIRSTNAME LIKE ?1 OR r.ZLASTNAME LIKE ?1
      OR (r.ZFIRSTNAME || ' ' || r.ZLASTNAME) LIKE ?1
      OR r.ZORGANIZATION LIKE ?1
    """

    case dbs(opts) do
      [] ->
        {:error, "no AddressBook database found (macOS Contacts; needs Full Disk Access)"}

      dbs ->
        dbs
        |> Enum.reduce_while({:ok, []}, fn db, {:ok, acc} ->
          case Sqlite.query(db, sql, ["%" <> name <> "%"]) do
            {:ok, rows} -> {:cont, {:ok, acc ++ group(rows)}}
            {:error, _} = err -> {:halt, err}
          end
        end)
        |> case do
          {:ok, people} ->
            {:ok, people |> Enum.uniq() |> Enum.take(Keyword.get(opts, :limit, 20))}

          err ->
            err
        end
    end
  end

  # The joins fan out to one row per (record, phone, email) combination;
  # fold them back into one map per person.
  defp group(rows) do
    rows
    |> Enum.group_by(& &1["pk"])
    |> Enum.map(fn {_pk, rows} ->
      first = hd(rows)

      %{
        name: Enum.join(Enum.reject([first["first"], first["last"]], &(&1 in [nil, ""])), " "),
        org: first["org"],
        phones: rows |> Enum.map(& &1["phone"]) |> Enum.reject(&is_nil/1) |> Enum.uniq(),
        emails: rows |> Enum.map(& &1["email"]) |> Enum.reject(&is_nil/1) |> Enum.uniq()
      }
    end)
    |> Enum.sort_by(& &1.name)
  end

  defp dbs(opts) do
    opts[:dbs] ||
      Path.wildcard(
        Path.expand("~/Library/Application Support/AddressBook/**/AddressBook-v22.abcddb")
      )
  end
end
