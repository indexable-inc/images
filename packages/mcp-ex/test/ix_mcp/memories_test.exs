defmodule IxMcp.MemoriesTest do
  # Mutates process-global env (MEMORIES_BIN, PATH), so never async.
  use ExUnit.Case, async: false

  alias IxMcp.Memories
  alias IxMcp.Memories.Diagnostic
  alias IxMcp.Memories.Hit
  alias IxMcp.Memories.Results
  alias IxMcp.Memories.Root
  alias IxMcp.Memories.Row
  alias IxMcp.Memories.Validation

  @mock Path.expand("../fixtures/memories_mock.exs", __DIR__)
  @env ~w(MEMORIES_BIN MEMORIES_MOCK_LOG)

  # Spawned through a /bin/sh trampoline, not its own shebang: a nix build
  # sandbox has no /usr/bin/env, and nixpkgs ships `bin/elixir` as a bash
  # script that no kernel will follow a second shebang into (the lesson
  # IxMcp.Memory.SemanticTest paid four days for).
  defp install_mock(dir) do
    interpreter =
      System.find_executable("elixir") ||
        raise "no elixir on PATH for the memories mock trampoline"

    path = Path.join(dir, "memories_mock")
    File.write!(path, "#!/bin/sh\nexec #{interpreter} #{@mock} \"$@\"\n")
    File.chmod!(path, 0o755)
    path
  end

  setup do
    previous = Map.new(@env, &{&1, System.get_env(&1)})
    dir = Path.join(System.tmp_dir!(), "ix-memories-test-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    log = Path.join(dir, "mock.log")
    System.put_env("MEMORIES_BIN", install_mock(dir))
    System.put_env("MEMORIES_MOCK_LOG", log)

    on_exit(fn ->
      for {key, value} <- previous do
        if value, do: System.put_env(key, value), else: System.delete_env(key)
      end

      File.rm_rf!(dir)
    end)

    {:ok, dir: dir, log: log}
  end

  test "a missing binary fails loudly naming the knob" do
    path = System.get_env("PATH")
    System.delete_env("MEMORIES_BIN")
    # An empty PATH is what makes the fallback miss: the knob's error only
    # fires when neither the env nor PATH resolves the binary.
    System.put_env("PATH", "")

    try do
      assert_raise RuntimeError, ~r/MEMORIES_BIN/, fn -> Memories.roots() end
    after
      if path, do: System.put_env("PATH", path)
    end
  end

  test "search decodes ranked hits and keeps the CLI's order", %{log: log} do
    results = Memories.search("why did every host rebuild")

    # The roots ride with the result: zero hits from one unexpected
    # directory is otherwise the same value as zero hits from the right
    # ones.
    assert %Results{query: "why did every host rebuild", scanned: 137, elapsed_ms: 8} = results

    # Roots are rows, not paths: the count is what separates "the right
    # directories, and they are empty" from "resolved somewhere unexpected".
    assert results.roots == [
             %Root{path: "/repo/.memories", exists: true, memories: 137},
             %Root{path: "/home/agent/.memories", exists: false, memories: 0}
           ]

    assert [stale, cascade] = results.hits

    # The mock emits ascending score AND ascending bm25, so this order
    # holds only because nothing here re-sorts what the CLI ranked.
    assert %Hit{slug: "hil-bench", score: 2.0, bm25: 5.0} = stale
    assert %Hit{slug: "nix-rebuild-cascade", score: 6.883, bm25: 7.412} = cascade

    assert cascade.path == "/repo/.memories/nix-rebuild-cascade.md"
    assert cascade.root == "/repo"
    assert cascade.tldr == "tldr for nix-rebuild-cascade"
    assert cascade.genre == :memory
    assert cascade.topic == ["nix"]
    assert cascade.handle == ["nix-dag"]
    assert cascade.prior == 0.8
    assert cascade.related == ["nix-eval-before-deploy"]
    assert cascade.supersedes == []
    assert cascade.refuted == false
    assert cascade.scope == "shared"
    assert cascade.body == "body of nix-rebuild-cascade"

    # A stale memory is returned flagged, never dropped, and a memory may
    # live one level deep in a grouping subdirectory.
    assert stale.path == "/repo/.memories/hardware/hil-bench.md"
    assert stale.root == "/repo"
    assert stale.genre == :living
    assert stale.stale
    assert stale.stale_reason == "based_on moved: packages/nix-dag/src/rank.rs"
    refute cascade.stale
    assert cascade.stale_reason == nil

    assert [%Validation{by: "claude-opus-4-6", ok: true} = receipt] = cascade.validated
    assert receipt.at == ~U[2026-07-29 18:22:11Z]
    assert receipt.how =~ "nix-dag"

    assert mock_events(log) == ["search --json -- why did every host rebuild"]
  end

  test "search flags repeat per value and the query stays positional", %{log: log} do
    assert %Results{hits: [_, _]} =
             Memories.search("-rebuild",
               limit: 5,
               topic: [:nix, "builds"],
               genre: :living,
               all: true,
               dirs: ["/one/.memories", "/two/.memories"]
             )

    assert mock_events(log) == [
             "search --dir /one/.memories --dir /two/.memories --json --limit 5 " <>
               "--topic nix --topic builds --genre living --all -- -rebuild"
           ]
  end

  test "roots reports the resolved default set as rows", %{log: log} do
    assert Memories.roots() == [
             %Root{path: "/repo/.memories", exists: true, memories: 137},
             %Root{path: "/home/agent/.memories", exists: false, memories: 0}
           ]

    # A root that is not there is reported, not dropped: a default root with
    # no `.memories` yet is normal, and the same row named explicitly is a typo.
    assert Enum.find(Memories.roots(), &(&1.path == "/home/agent/.memories")).exists == false
    assert mock_events(log) == ["roots --json", "roots --json"]
  end

  test "show carries no ranking fields", %{log: log} do
    hit = Memories.show("nix-rebuild-cascade", dirs: ["/one/.memories"])

    assert %Hit{slug: "nix-rebuild-cascade", bm25: nil, score: nil} = hit
    assert hit.tldr == "tldr for nix-rebuild-cascade"
    assert mock_events(log) == ["show --dir /one/.memories --json -- nix-rebuild-cascade"]
  end

  test "dirs: takes a list and only a list", %{log: log} do
    # One spelling: a bare string is refused rather than wrapped, so the
    # plural path is the only path callers can take.
    assert_raise ArgumentError, ~r/dirs: expects a list/, fn ->
      Memories.search("rebuild", dirs: "/one/.memories")
    end

    assert_raise ArgumentError, ~r/in the list/, fn ->
      Memories.search("rebuild", dirs: [:one])
    end

    # Refused before the spawn, not after.
    assert mock_events(log) == []
  end

  test "a slug that does not resolve raises with the CLI's own message" do
    assert_raise RuntimeError, ~r/no memory named no-such-memory/, fn ->
      Memories.show("no-such-memory")
    end
  end

  test "the review commands decode rows", %{log: log} do
    assert [%Row{slug: "hil-bench", path: "/repo/.memories/hil-bench.md"} = row] =
             Memories.stale()

    assert row.tldr == "tldr for hil-bench"
    assert row.reason == "stale: nobody re-ran the proof"

    assert [%Row{reason: "refuted: nobody re-ran the proof"}] = Memories.refuted()
    assert [%Row{reason: "unchecked: nobody re-ran the proof"}] = Memories.unchecked(days: 30)

    # --days is passed only when given: the CLI owns the default.
    assert [%Row{}] = Memories.unchecked()

    assert mock_events(log) == [
             "stale --json",
             "refuted --json",
             "unchecked --json --days 30",
             "unchecked --json"
           ]
  end

  test "lint returns its report on the exit-1 path", %{log: log} do
    report = Memories.lint()

    assert report.errors == 2
    assert report.checked == 137

    assert [%Diagnostic{rule: "memory-topic-unknown", line: 3} = first, second] =
             report.diagnostics

    assert first.path == "/repo/.memories/hil-bench.md"
    assert first.message =~ "closed set"
    # `line` is absent for a whole-file rule.
    assert %Diagnostic{rule: "memory-tldr", line: nil} = second

    assert mock_events(log) == ["lint --json"]
  end

  test "remember builds every flag and sends the body on stdin", %{log: log} do
    assert :ok =
             Memories.remember("nix-rebuild-cascade", "an env var holding a store path",
               body: "why:\nthe evidence\n",
               genre: :living,
               topic: [:nix, :builds],
               handle: ~w(nix-dag drvPath),
               prior: 0.8,
               related: ["nix-eval-before-deploy"],
               based_on: ["packages/nix-dag/src/rank.rs"],
               scope: "user:andrew"
             )

    assert mock_events(log) == [
             "remember --tldr an env var holding a store path --genre living " <>
               "--topic nix --topic builds --handle nix-dag --handle drvPath " <>
               "--prior 0.8 --related nix-eval-before-deploy " <>
               "--based-on packages/nix-dag/src/rank.rs --scope user:andrew " <>
               "-- nix-rebuild-cascade",
             "body why:\\nthe evidence\\n"
           ]
  end

  test "a body-less remember still reaches the CLI with empty stdin", %{log: log} do
    assert :ok = Memories.remember("loops", "while loops are tricky")

    assert mock_events(log) == [
             "remember --tldr while loops are tricky -- loops",
             "body "
           ]
  end

  test "validate records the failing check too, and refute names the replacement", %{log: log} do
    assert :ok = Memories.validate("hil-bench", by: "claude-opus-5", how: "ssh hil; uptime")
    assert :ok = Memories.validate("hil-bench", by: "claude-opus-5", how: "ssh hil", ok: false)

    assert :ok =
             Memories.refute("hil-bench",
               by: "claude-opus-5",
               how: "the bench moved",
               instead: "hil-bench-2"
             )

    assert mock_events(log) == [
             "validate --by claude-opus-5 --how ssh hil; uptime -- hil-bench",
             "validate --by claude-opus-5 --how ssh hil --not-ok -- hil-bench",
             "refute --by claude-opus-5 --how the bench moved --instead hil-bench-2 -- hil-bench"
           ]
  end

  test "validate demands the command that proves it" do
    assert_raise KeyError, fn -> Memories.validate("hil-bench", by: "claude-opus-5") end
  end

  test "expand appends related neighbours from the referrer's own root", %{log: log} do
    hits = Memories.search("rebuild").hits
    expanded = Memories.expand(hits)

    assert Enum.map(expanded, & &1.slug) == [
             "hil-bench",
             "nix-rebuild-cascade",
             "loops",
             "nix-eval-before-deploy"
           ]

    # Both neighbours are read from the `.memories` ROOT their referrer came
    # from, never from `Path.dirname(path)`: hil-bench sits in the `hardware/`
    # group, and passing that group as a root would hide every sibling group.
    assert mock_events(log) == [
             "search --json -- rebuild",
             "show --dir /repo/.memories --json -- loops",
             "show --dir /repo/.memories --json -- nix-eval-before-deploy"
           ]

    # depth: 0 is the identity, and a second pass fetches nothing new
    # (the mock's neighbour has no `related:` of its own).
    assert Memories.expand(hits, depth: 0) == hits
    assert Memories.expand(expanded, depth: 2) == expanded
  end

  defp mock_events(log) do
    case File.read(log) do
      {:ok, content} -> String.split(content, "\n", trim: true)
      {:error, :enoent} -> []
    end
  end
end
