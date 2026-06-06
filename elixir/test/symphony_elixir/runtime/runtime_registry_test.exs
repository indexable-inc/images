defmodule SymphonyElixir.Runtime.RuntimeRegistryTest do
  use ExUnit.Case, async: false

  alias SymphonyElixir.Runtime.RuntimeRegistry

  setup do
    start_supervised!(RuntimeRegistry)
    :ok
  end

  defp worker(id, overrides \\ %{}) do
    Map.merge(
      %{worker_id: id, pid: self(), address: "100.0.0.1", labels: ["default"], capacity: 4},
      overrides
    )
  end

  test "register makes a worker discoverable by get/list/select" do
    :ok = RuntimeRegistry.register(worker("w1"))

    assert {:ok, %{worker_id: "w1", address: "100.0.0.1", labels: ["default"]}} = RuntimeRegistry.get("w1")
    assert [%{worker_id: "w1"}] = RuntimeRegistry.list()
    assert {:ok, %{worker_id: "w1"}} = RuntimeRegistry.select()
  end

  test "get is :error for an unknown worker" do
    assert :error = RuntimeRegistry.get("nope")
  end

  test "select filters by label and returns :no_worker when none match" do
    :ok = RuntimeRegistry.register(worker("w1", %{labels: ["us-west"]}))
    :ok = RuntimeRegistry.register(worker("w2", %{labels: ["hari"]}))

    assert {:ok, %{worker_id: "w2"}} = RuntimeRegistry.select("hari")
    assert {:error, :no_worker} = RuntimeRegistry.select("nonexistent")
  end

  test "select returns :no_worker when the registry is empty" do
    assert {:error, :no_worker} = RuntimeRegistry.select()
  end

  test "re-registering the same id replaces the prior entry" do
    :ok = RuntimeRegistry.register(worker("w1", %{address: "100.0.0.1"}))
    :ok = RuntimeRegistry.register(worker("w1", %{address: "100.0.0.9"}))

    assert {:ok, %{address: "100.0.0.9"}} = RuntimeRegistry.get("w1")
    assert [_one] = RuntimeRegistry.list()
  end

  test "unregister drops a worker" do
    :ok = RuntimeRegistry.register(worker("w1"))
    :ok = RuntimeRegistry.unregister("w1")
    assert :error = RuntimeRegistry.get("w1")
  end

  test "a worker whose channel process dies is dropped automatically" do
    parent = self()
    pid = spawn(fn -> receive do: (:stop -> send(parent, :stopped)) end)
    :ok = RuntimeRegistry.register(worker("w1", %{pid: pid}))
    assert {:ok, _} = RuntimeRegistry.get("w1")

    Process.exit(pid, :kill)
    assert eventually(fn -> RuntimeRegistry.get("w1") == :error end)
  end

  defp eventually(fun, retries \\ 50) do
    cond do
      fun.() ->
        true

      retries == 0 ->
        false

      true ->
        Process.sleep(10)
        eventually(fun, retries - 1)
    end
  end
end
