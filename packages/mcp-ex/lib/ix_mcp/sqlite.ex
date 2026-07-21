defmodule IxMcp.Sqlite do
  @moduledoc """
  One-shot read-only queries against local SQLite files (Messages,
  AddressBook) over the exqlite NIF the action log already ships: rows
  come back as maps keyed by column name, parameters bind properly, and
  nothing shells out.
  """

  alias Exqlite.Sqlite3

  @doc "Run `sql` against `db` read-only with `params` bound."
  @spec query(Path.t(), String.t(), list()) :: {:ok, [map()]} | {:error, String.t()}
  def query(db, sql, params \\ []) do
    # :readonly still creates an empty file for a missing path on some
    # platforms; an explicit check keeps the message honest either way.
    with true <- File.exists?(db) || {:error, "#{db}: no such database"},
         {:ok, conn} <- wrap(db, Sqlite3.open(db, mode: :readonly)) do
      try do
        with {:ok, stmt} <- wrap(db, Sqlite3.prepare(conn, sql)),
             :ok <- wrap(db, Sqlite3.bind(stmt, params)),
             {:ok, cols} <- wrap(db, Sqlite3.columns(conn, stmt)),
             {:ok, rows} <- wrap(db, Sqlite3.fetch_all(conn, stmt)) do
          {:ok, Enum.map(rows, &Map.new(Enum.zip(cols, &1)))}
        end
      after
        Sqlite3.close(conn)
      end
    end
  end

  defp wrap(_db, :ok), do: :ok
  defp wrap(_db, {:ok, value}), do: {:ok, value}
  defp wrap(db, {:error, reason}), do: {:error, "#{db}: #{inspect(reason)}"}
end
