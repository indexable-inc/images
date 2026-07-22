# Shared ExUnit support, staged by unibind's ex target
# (packages/unibind/nix/ex.nix) into every binding suite as
# test/support/eventually.exs and loaded from test_helper.exs. Runtime
# observables behind a NIF (cancellation, GC, PTY exits) are asynchronous
# on the BEAM side, so assertions poll with a grace window instead of
# sleeping a fixed amount.
defmodule UnibindTest.Eventually do
  @doc "Polls `fun` until it returns true or `timeout_ms` elapses."
  def eventually(fun, timeout_ms \\ 2_000) do
    deadline = System.monotonic_time(:millisecond) + timeout_ms
    poll(fun, deadline)
  end

  defp poll(fun, deadline) do
    cond do
      fun.() ->
        true

      System.monotonic_time(:millisecond) > deadline ->
        false

      true ->
        Process.sleep(20)
        poll(fun, deadline)
    end
  end
end
