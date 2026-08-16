defmodule FleetMesh.ClickHouseTest do
  use ExUnit.Case, async: false

  alias FleetMesh.ClickHouse

  # async: false: mutates IX_CLICKHOUSE_HOST and the :fleet_mesh app env.

  setup do
    original = System.get_env("IX_CLICKHOUSE_HOST")
    original_app = Application.get_env(:fleet_mesh, :clickhouse_host)

    on_exit(fn ->
      case original do
        nil -> System.delete_env("IX_CLICKHOUSE_HOST")
        value -> System.put_env("IX_CLICKHOUSE_HOST", value)
      end

      case original_app do
        nil -> Application.delete_env(:fleet_mesh, :clickhouse_host)
        value -> Application.put_env(:fleet_mesh, :clickhouse_host, value)
      end
    end)

    :ok
  end

  test "env wins over app config" do
    System.put_env("IX_CLICKHOUSE_HOST", "leader-from-env")
    Application.put_env(:fleet_mesh, :clickhouse_host, "leader-from-config")
    assert ClickHouse.host() == "leader-from-env"
  end

  test "app config answers when the env is unset" do
    System.delete_env("IX_CLICKHOUSE_HOST")
    Application.put_env(:fleet_mesh, :clickhouse_host, "leader-from-config")
    assert ClickHouse.host() == "leader-from-config"
  end

  test "nothing configured raises with the fix in the message" do
    System.delete_env("IX_CLICKHOUSE_HOST")
    Application.delete_env(:fleet_mesh, :clickhouse_host)

    assert_raise RuntimeError, ~r/IX_CLICKHOUSE_HOST/, fn -> ClickHouse.host() end
  end
end
