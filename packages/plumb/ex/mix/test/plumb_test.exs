defmodule PlumbTest do
  # Not async: evals share one OS process's children; keep runs ordered so
  # the run ids in ${o[N]} assertions stay deterministic per shell.
  use ExUnit.Case, async: false

  defp eventually(fun, timeout_ms \\ 2_000) do
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

  defp decode!({:ok, json}), do: JSON.decode!(json)

  defp final_stdout(report) do
    report["pipelines"]
    |> List.last()
    |> Map.fetch!("stages")
    |> List.last()
    |> get_in(["stdout", "text"])
  end

  test "eval returns a report with per-stage captures" do
    {:ok, shell} = Plumb.Shell.new()
    report = decode!(Plumb.Shell.eval(shell, "echo hello | tr a-z A-Z"))
    assert report["status"] == 0
    stages = hd(report["pipelines"])["stages"]
    assert length(stages) == 2
    assert hd(stages)["stdout"]["text"] == "hello\n"
    assert final_stdout(report) == "HELLO\n"
  end

  test "auto-bound variables cross evals and are readable from Elixir" do
    {:ok, shell} = Plumb.Shell.new()
    {:ok, _} = Plumb.Shell.eval(shell, "echo alpha")
    report = decode!(Plumb.Shell.eval(shell, "echo ${o[1]}-more"))
    assert final_stdout(report) == "alpha-more\n"
    # Bare $o is the most recently finished run (#3595); earlier runs stay
    # addressable through ${o[N]}, exercised by the eval above.
    assert Plumb.Shell.var(shell, "o") == "alpha-more"
    assert Plumb.Shell.var(shell, "does-not-exist") == nil
  end

  test "set_var round-trips into command expansion" do
    {:ok, shell} = Plumb.Shell.new()
    Plumb.Shell.set_var(shell, "GREETING", "from-elixir")
    report = decode!(Plumb.Shell.eval(shell, "echo $GREETING"))
    assert final_stdout(report) == "from-elixir\n"
  end

  test "parse errors carry the :parse variant" do
    {:ok, shell} = Plumb.Shell.new()

    assert {:error, %Plumb.PlumbError{variant: :parse}} =
             Plumb.Shell.eval(shell, "echo `date`")
  end

  test "strictness errors carry the :strict variant" do
    {:ok, shell} = Plumb.Shell.new()

    assert {:error, %Plumb.PlumbError{variant: :strict, message: message}} =
             Plumb.Shell.eval(shell, "echo $DEFINITELY_UNSET_VARIABLE")

    assert message =~ "DEFINITELY_UNSET_VARIABLE"
  end

  test "exit surfaces as :exit" do
    {:ok, shell} = Plumb.Shell.new()
    assert {:error, %Plumb.PlumbError{variant: :exit}} = Plumb.Shell.eval(shell, "exit 3")
  end

  test "nonzero command status is report data, not an error" do
    {:ok, shell} = Plumb.Shell.new()
    report = decode!(Plumb.Shell.eval(shell, "false"))
    assert report["status"] == 1
    assert Plumb.Shell.last_status(shell) == 1
  end

  test "detached runs commit a report the shell can fetch later" do
    {:ok, shell} = Plumb.Shell.new()
    id = Plumb.Shell.eval_start(shell, "echo detached")

    assert eventually(fn ->
             match?({:ok, json} when is_binary(json), Plumb.Shell.report(shell, id))
           end),
           "detached run never committed"

    {:ok, json} = Plumb.Shell.report(shell, id)
    assert final_stdout(JSON.decode!(json)) == "detached\n"
    assert id in Plumb.Shell.run_ids(shell)
  end

  test "cwd is exposed" do
    {:ok, shell} = Plumb.Shell.new()
    assert is_binary(Plumb.Shell.cwd(shell))
    assert String.starts_with?(Plumb.Shell.cwd(shell), "/")
  end

  test "one-shot run/1" do
    report = decode!(Plumb.run("echo -n one-shot"))
    assert final_stdout(report) == "one-shot"
  end
end
