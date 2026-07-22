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

  # The module is a thin shell over the weave CLI, so the round-trip
  # coverage needs the real binary: it runs on dev machines and compiles
  # away in the sandboxed nix check, where weave is not staged.
  if System.find_executable("weave") do
    describe "against a throwaway store" do
      alias IxMcp.Memory

      setup do
        store =
          Path.join(System.tmp_dir!(), "ix-memory-test-#{System.unique_integer([:positive])}")

        {_, 0} = System.cmd("weave", ["--store", store, "init"])
        previous = System.get_env("WEAVE_MEMORY_STORE")
        System.put_env("WEAVE_MEMORY_STORE", store)

        on_exit(fn ->
          case previous do
            nil -> System.delete_env("WEAVE_MEMORY_STORE")
            _ -> System.put_env("WEAVE_MEMORY_STORE", previous)
          end

          File.rm_rf!(store)
        end)

        :ok
      end

      test "recall matches whole words and returns rich rows" do
        Memory.remember("loops", "while loops are tricky")

        Memory.remember("hil-rig", "hil bench notes",
          type: "reference",
          topic: ["hardware", "lab"],
          handle: "ssh hil",
          body: "long-form hil notes"
        )

        # Word-boundary default: "hil" must not match "while".
        assert [row] = Memory.recall("hil")
        assert row.entity == "mem:hil-rig"
        assert row.desc == "hil bench notes"
        assert row.type == "reference"
        assert row.topic == ["hardware", "lab"]
        assert row.handle == "ssh hil"
        assert row.body == "long-form hil notes"
        assert row.verified_at == nil
        assert String.starts_with?(row.id, "blake3:")
        assert is_integer(row.seq)
        assert %DateTime{} = row.time

        substring = Memory.recall("hil", match: :substring)
        assert substring |> Enum.map(& &1.entity) |> Enum.sort() == ["mem:hil-rig", "mem:loops"]

        assert [_only] = Memory.recall("notes|tricky", limit: 1)
      end

      test "supersedes/relates become typed edges that graph walks" do
        Memory.remember("old", "stale advice")
        Memory.remember("peer", "adjacent note")
        Memory.remember("new", "fresh advice", supersedes: "old", relates: ["peer"])

        assert Enum.sort(Memory.graph("new")) == [
                 %{from: "mem:new", edge: "mem/relates", to: "mem:peer"},
                 %{from: "mem:new", edge: "mem/supersedes", to: "mem:old"}
               ]

        # The neighborhood is bidirectional: the superseded side sees the edge.
        assert Memory.graph("old") == [
                 %{from: "mem:new", edge: "mem/supersedes", to: "mem:old"}
               ]
      end

      test "verify appends a receipt that recall surfaces" do
        Memory.remember("claim", "checkable claim")
        id = Memory.verify("claim", session: "sess-42")
        assert String.starts_with?(id, "blake3:")

        assert [row] = Memory.recall("checkable")
        assert row.verified_at =~ ~r/^\d{4}-\d{2}-\d{2}T[0-9:.]+Z sess-42$/
      end
    end
  end
end
