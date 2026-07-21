defmodule IxMcp.MemoryTest do
  # Mutates process-global env, so never async.
  use ExUnit.Case, async: false

  test "unset WEAVE_MEMORY_STORE fails loudly with the setup knob" do
    previous = System.get_env("WEAVE_MEMORY_STORE")
    System.delete_env("WEAVE_MEMORY_STORE")

    try do
      assert_raise RuntimeError, ~r/WEAVE_MEMORY_STORE/, fn ->
        IxMcp.Memory.query("?- latest(E, A, V).")
      end
    after
      if previous, do: System.put_env("WEAVE_MEMORY_STORE", previous)
    end
  end
end
