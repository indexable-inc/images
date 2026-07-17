defmodule IxMcp.Evaluator.IOProxy do
  @moduledoc """
  A group leader for one job: an Erlang IO device that appends every
  `put_chars` request to the job's output buffer instead of the real stdout.

  Installing it as group leader does double duty. First, output capture:
  `IO.puts` in a cell, and in every process the cell spawns, lands in the
  job's pageable buffer. Second, process tagging: the group leader is
  inherited by spawned processes, so "every process this job created" is
  answerable by scanning `Process.list/0` for this group leader -- which is
  how cancellation finds the OS processes a job's cells spawned (see
  `IxMcp.OsProc`).

  Input requests answer `:eof`: cells are non-interactive, exactly like the
  Python kernel.

  The sink's contract is a valid UTF-8 binary, nothing else: chunks feed
  `byte_size/1` sums in job summaries and ride tool replies as JSON. See
  `convert/2` (#3538).
  """

  alias IxMcp.UTF8

  @spec start_link((iodata() -> any())) :: {:ok, pid()}
  def start_link(sink) when is_function(sink, 1) do
    {:ok, spawn_link(fn -> loop(sink) end)}
  end

  defp loop(sink) do
    receive do
      {:io_request, from, reply_as, request} ->
        send(from, {:io_reply, reply_as, handle(request, sink)})
        loop(sink)

      _other ->
        loop(sink)
    end
  end

  defp handle({:put_chars, encoding, chars}, sink) do
    sink.(convert(chars, encoding))
    :ok
  rescue
    _ -> {:error, :put_chars}
  end

  defp handle({:put_chars, encoding, mod, fun, args}, sink) do
    handle({:put_chars, encoding, apply(mod, fun, args)}, sink)
  end

  defp handle({:requests, requests}, sink) do
    Enum.reduce_while(requests, :ok, fn request, :ok ->
      case handle(request, sink) do
        :ok -> {:cont, :ok}
        error -> {:halt, error}
      end
    end)
  end

  defp handle({:get_chars, _enc, _prompt, _count}, _sink), do: :eof
  defp handle({:get_line, _enc, _prompt}, _sink), do: :eof
  defp handle({:get_until, _enc, _prompt, _mod, _fun, _args}, _sink), do: :eof
  defp handle({:setopts, _opts}, _sink), do: :ok
  defp handle(:getopts, _sink), do: [binary: true, encoding: :unicode]
  defp handle({:get_geometry, _}, _sink), do: {:error, :enotsup}
  defp handle(_request, _sink), do: {:error, :request}

  # `:unicode.characters_to_binary/1` answers invalid UTF-8 with an
  # {:error, ...} TUPLE, not a binary. Passed through unchecked, that tuple
  # landed in the job's output buffer as if it were output, `byte_size/1`
  # crashed the job GenServer mid-finish, and the client hung forever on a
  # request nothing would ever answer -- #3538, a cell printing a compiled
  # binary. UTF8.sanitize/1 keeps valid UTF-8 byte-identical and turns
  # every invalid byte into a visible `\xNN` escape, so the sink invariant
  # (valid UTF-8 binary, always) holds no matter what a cell prints.
  defp convert(chars, :unicode), do: UTF8.sanitize(chars)

  # :latin1 put_chars (IO.binwrite and friends) hands over raw bytes. Bytes
  # that already form valid UTF-8 pass byte-identical -- the same
  # transparency #3523 gave the wire -- and anything else is escaped rather
  # than reinterpreted as Latin-1 text: what actually reaches this clause is
  # binary dumps, and mapping them to accented letters would disguise the
  # bytes instead of naming them (#3538).
  defp convert(chars, :latin1), do: chars |> :erlang.iolist_to_binary() |> UTF8.sanitize()
end
