defmodule Sample.Native do
  @moduledoc false

  @app :sample

  @on_load :__load_nif__

  def __load_nif__ do
    :code.priv_dir(@app)
    |> to_string()
    |> Path.join("native/libsample")
    |> String.to_charlist()
    |> :erlang.load_nif(0)
  end

  def rows(_store, _limit, _root), do: :erlang.nif_error(:not_loaded)

  def recount(_home), do: :erlang.nif_error(:not_loaded)

  def label_of(_ref, _id), do: :erlang.nif_error(:not_loaded)

  def store(_ref, _row), do: :erlang.nif_error(:not_loaded)

  def tags(_ref, _prefix), do: :erlang.nif_error(:not_loaded)

  def scan(_ref, _store), do: :erlang.nif_error(:not_loaded)

  def cursor_open(_store), do: :erlang.nif_error(:not_loaded)

  def cursor_position(_handle), do: :erlang.nif_error(:not_loaded)

  def unibind_demand(_handle, _n), do: :erlang.nif_error(:not_loaded)
end

