defmodule FleetMesh.ConditionTest do
  use ExUnit.Case, async: true

  alias FleetMesh.Condition

  @valid [
    id: :sample,
    severity: :warning,
    description: "a sample",
    interval_ms: 50,
    check: {__MODULE__, :green_check, []}
  ]

  def green_check, do: :green

  test "new/1 builds a condition from valid attrs" do
    condition = Condition.new(@valid)
    assert condition.id == :sample
    assert condition.interval_ms == 50
  end

  test "new/1 refuses each malformed field by name" do
    for {key, bad} <- [
          id: "sample",
          severity: :loud,
          description: nil,
          interval_ms: 0,
          check: {:not, :an, "mfa"}
        ] do
      assert_raise ArgumentError, ~r/#{key}/, fn ->
        Condition.new(Keyword.put(@valid, key, bad))
      end
    end
  end

  test "from_query/2 maps reader results onto the fixed state mapping" do
    build = fn reader ->
      [id: :q, severity: :error, description: "q", interval_ms: 50]
      |> Condition.from_query(reader)
      |> Condition.evaluate()
    end

    assert build.(fn -> {:ok, []} end) == {:green, []}
    assert build.(fn -> {:ok, [:hit]} end) == {:red, [:hit]}
    assert build.(fn -> {:error, :down} end) == {:unknown, :down}
    assert {:unknown, {:unexpected_reader_return, 42}} = build.(fn -> 42 end)
  end

  test "evaluate/1 turns a raising check into :unknown, not a crash" do
    condition = Condition.new(Keyword.put(@valid, :check, fn -> raise "boom" end))

    assert {:unknown, {:error, %RuntimeError{message: "boom"}, _stack}} =
             Condition.evaluate(condition)
  end

  test "evaluate/1 normalises bare states and rejects other shapes" do
    bare = Condition.new(Keyword.put(@valid, :check, fn -> :red end))
    assert Condition.evaluate(bare) == {:red, nil}

    odd = Condition.new(Keyword.put(@valid, :check, fn -> {:sideways, 1} end))
    assert {:unknown, {:unexpected_check_return, {:sideways, 1}}} = Condition.evaluate(odd)
  end
end
