defmodule IxMcp.FleetTest do
  use ExUnit.Case, async: false

  alias IxMcp.Fleet

  @nodes_env "IX_BEAM_NODES"

  setup do
    original = System.get_env(@nodes_env)

    on_exit(fn ->
      case original do
        nil -> System.delete_env(@nodes_env)
        value -> System.put_env(@nodes_env, value)
      end
    end)

    :ok
  end

  describe "nodes/0" do
    test "returns [] when IX_BEAM_NODES is unset" do
      System.delete_env(@nodes_env)
      assert Fleet.nodes() == []
    end

    test "returns [] when IX_BEAM_NODES is empty" do
      System.put_env(@nodes_env, "")
      assert Fleet.nodes() == []
    end

    test "splits on commas, spaces, tabs and newlines" do
      System.put_env(@nodes_env, "beamd@a.ts.net, beamd@b.ts.net\tbeamd@c.ts.net\nbeamd@d.ts.net")

      assert Fleet.nodes() == [
               :"beamd@a.ts.net",
               :"beamd@b.ts.net",
               :"beamd@c.ts.net",
               :"beamd@d.ts.net"
             ]
    end
  end

  describe "dispatch with no configured nodes" do
    setup do
      System.delete_env(@nodes_env)
      :ok
    end

    # These short-circuit on an empty node list *before* touching
    # distribution, so the suite never opens an epmd socket (which the
    # sandboxed build forbids).
    test "exec_any/2 reports :no_nodes" do
      assert Fleet.exec_any("1 + 1") == {:error, :no_nodes}
    end

    test "exec_least_loaded/2 reports :no_nodes" do
      assert Fleet.exec_least_loaded("1 + 1") == {:error, :no_nodes}
    end

    test "zero-arity funs pass the code guard" do
      assert Fleet.exec_any(fn -> 1 + 1 end) == {:error, :no_nodes}
      assert Fleet.exec_least_loaded(fn -> 1 + 1 end) == {:error, :no_nodes}
    end

    test "compat_error is :ok on matching md5 and instructive on divergence" do
      md5 = :erl_eval.module_info(:md5)
      assert Fleet.compat_error(md5, md5, :"beamd@a.ts.net") == :ok

      assert {:error, {:erl_eval_mismatch, msg}} =
               Fleet.compat_error(md5, <<0::128>>, :"beamd@a.ts.net")

      assert msg =~ "erlang pins have diverged"
      assert msg =~ "beamd@a.ts.net"
    end

    test "non-code payloads are rejected" do
      assert_raise FunctionClauseError, fn -> Fleet.exec_any(~c"1 + 1") end
      assert_raise FunctionClauseError, fn -> Fleet.exec_any(fn x -> x end) end
    end
  end
end
