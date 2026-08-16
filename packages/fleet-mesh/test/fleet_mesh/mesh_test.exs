defmodule FleetMesh.MeshTest do
  use ExUnit.Case, async: false

  alias FleetMesh.Mesh

  # async: false: these mutate IX_BEAM_NODES, which is process-global.

  setup do
    original = System.get_env("IX_BEAM_NODES")

    on_exit(fn ->
      case original do
        nil -> System.delete_env("IX_BEAM_NODES")
        value -> System.put_env("IX_BEAM_NODES", value)
      end
    end)

    :ok
  end

  test "nodes/0 parses the separators the deploy contract names" do
    System.put_env("IX_BEAM_NODES", "beamd@a.ts.net, beamd@b.ts.net\nbeamd@c.ts.net")
    assert Mesh.nodes() == [:"beamd@a.ts.net", :"beamd@b.ts.net", :"beamd@c.ts.net"]
  end

  test "nodes/0 is [] when unset" do
    System.delete_env("IX_BEAM_NODES")
    assert Mesh.nodes() == []
  end

  test "exec_any/2 reports :no_nodes for both code shapes" do
    System.delete_env("IX_BEAM_NODES")
    assert Mesh.exec_any("1 + 1") == {:error, :no_nodes}
    assert Mesh.exec_any(fn -> 1 + 1 end) == {:error, :no_nodes}
  end

  test "exec_least_loaded/2 reports :no_nodes" do
    System.delete_env("IX_BEAM_NODES")
    assert Mesh.exec_least_loaded("1 + 1") == {:error, :no_nodes}
  end

  test "code guard rejects charlists and wrong-arity funs" do
    assert_raise FunctionClauseError, fn -> Mesh.exec_any(~c"1 + 1") end
    assert_raise FunctionClauseError, fn -> Mesh.exec_any(fn x -> x end) end
  end

  test "compat_error is :ok on matching md5 and instructive on divergence" do
    md5 = :erl_eval.module_info(:md5)
    assert Mesh.compat_error(md5, md5, :"beamd@a.ts.net") == :ok

    assert {:error, {:erl_eval_mismatch, message}} =
             Mesh.compat_error(md5, <<0::128>>, :"beamd@a.ts.net")

    assert message =~ "erlang pins have diverged"
  end
end
