#!/usr/bin/env elixir
# Mock `memories` CLI for IxMcp.MemoriesTest: enough of the --json
# contract (packages/memories/CONTRACT.md) to pin the Elixir wrapper's
# argv construction, decoding, and exit-code handling without the real
# binary, which is a Rust build this package does not depend on.
# Appends one line per invocation to $MEMORIES_MOCK_LOG -- the argv, and
# for `remember` the body it read on stdin -- so a test can assert what
# was actually spawned. An .exs rather than a shell script: new shell is
# fenced (#3823).
#
# This mock is a CLI stand-in: stdout is its whole interface, not a
# debugging leftover, so the IO.puts gate does not apply here.
# credo:disable-for-this-file Credo.Check.Refactor.IoPuts
defmodule MemoriesMock do
  @receipt %{
    "at" => "2026-07-29T18:22:11Z",
    "by" => "claude-opus-4-6",
    "how" => "nix-dag .#hil-compute-2; top sole-count node was IX_ASSETS_DIR",
    "ok" => true
  }

  # Resolved root rows, not paths: one that holds memories and one that is
  # not there, so a decoder that flattened them would lose the counts.
  def roots do
    [
      %{"path" => "/repo/.memories", "exists" => true, "memories" => 137},
      %{"path" => "/home/agent/.memories", "exists" => false, "memories" => 0}
    ]
  end

  def log(line) do
    case System.get_env("MEMORIES_MOCK_LOG") do
      nil -> :ok
      path -> File.write!(path, line <> "\n", [:append])
    end
  end

  # The `search` hit shape; `show` drops the two ranking keys.
  def hit(slug, overrides) do
    Map.merge(
      %{
        "slug" => slug,
        "path" => "/repo/.memories/#{slug}.md",
        "root" => "/repo",
        "tldr" => "tldr for #{slug}",
        "genre" => "memory",
        "topic" => ["nix"],
        "handle" => ["nix-dag"],
        "prior" => 0.8,
        "related" => [],
        "supersedes" => [],
        "scope" => "shared",
        "bm25" => 1.0,
        "score" => 1.0,
        "stale" => false,
        "stale_reason" => nil,
        "refuted" => false,
        "validated" => [@receipt],
        "body" => "body of #{slug}"
      },
      overrides
    )
  end

  def run(["search" | _] = args) do
    log(Enum.join(args, " "))

    # Emitted in the CLI's ranked order, which here is ascending in both
    # `score` and `bm25`: a wrapper that sorted either way would reverse
    # this, so the test's order assertion proves it does not.
    hits = [
      # One level deep in a grouping subdirectory, which is NOT a root:
      # `path` carries the group, `root` still names the project.
      hit("hil-bench", %{
        "path" => "/repo/.memories/hardware/hil-bench.md",
        "genre" => "living",
        "bm25" => 5.0,
        "score" => 2.0,
        "stale" => true,
        "stale_reason" => "based_on moved: packages/nix-dag/src/rank.rs",
        "topic" => ["hardware", "lab"],
        "related" => ["loops"]
      }),
      hit("nix-rebuild-cascade", %{
        "bm25" => 7.412,
        "score" => 6.883,
        "related" => ["nix-eval-before-deploy"]
      })
    ]

    emit(%{
      "query" => List.last(args),
      "roots" => roots(),
      "scanned" => 137,
      "elapsed_ms" => 8,
      "hits" => hits
    })
  end

  def run(["roots" | _] = args) do
    log(Enum.join(args, " "))
    emit(%{"roots" => roots()})
  end

  def run(["show" | _] = args) do
    log(Enum.join(args, " "))
    slug = List.last(args)

    case slug do
      "no-such-memory" ->
        IO.puts(:stderr, "memories: no memory named #{slug}")
        System.halt(1)

      _ ->
        emit(Map.drop(hit(slug, %{}), ["bm25", "score"]))
    end
  end

  def run([review | _] = args) when review in ["stale", "refuted", "unchecked"] do
    log(Enum.join(args, " "))

    emit(%{
      "rows" => [
        %{
          "slug" => "hil-bench",
          "path" => "/repo/.memories/hil-bench.md",
          "tldr" => "tldr for hil-bench",
          "reason" => "#{review}: nobody re-ran the proof"
        }
      ]
    })
  end

  # Exit 1 with a report on stdout: a lint error is an outcome, not a
  # failure to run.
  def run(["lint" | _] = args) do
    log(Enum.join(args, " "))

    emit(%{
      "diagnostics" => [
        %{
          "path" => "/repo/.memories/hil-bench.md",
          "line" => 3,
          "rule" => "memory-topic-unknown",
          "message" => "topic \"nixos\" is not in the closed set"
        },
        %{
          "path" => "/repo/.memories/loops.md",
          "rule" => "memory-tldr",
          "message" => "tldr is missing"
        }
      ],
      "errors" => 2,
      "checked" => 137
    })

    System.halt(1)
  end

  def run(["remember" | _] = args) do
    log(Enum.join(args, " "))
    log("body #{body(IO.read(:stdio, :eof))}")
  end

  def run([write | _] = args) when write in ["validate", "refute"] do
    log(Enum.join(args, " "))
  end

  def run(args) do
    IO.puts(:stderr, "mock memories: unhandled args: #{inspect(args)}")
    System.halt(2)
  end

  defp body(:eof), do: ""
  defp body(content), do: String.replace(content, "\n", "\\n")

  defp emit(payload), do: IO.puts(JSON.encode!(payload))
end

MemoriesMock.run(System.argv())
