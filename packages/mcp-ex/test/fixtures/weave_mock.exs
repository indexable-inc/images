#!/usr/bin/env elixir
# Mock weave for IxMcp.Memory.Semantic plumbing tests: enough of the
# `recall --stdin` NDJSON protocol and the two `query` programs
# Memory.semantic/2 issues to run without the real binary (whose default
# model is a 2.4GB download). Appends "spawn <query>" / "line <query>" to
# $WEAVE_MOCK_LOG so tests can count process starts vs resident answers.
# An .exs rather than a shell script: new shell is fenced (#3823).
#
# This mock is a CLI stand-in: stdout is its whole interface, not a
# debugging leftover, so the IO.puts gate does not apply here.
# credo:disable-for-this-file Credo.Check.Refactor.IoPuts
defmodule WeaveMock do
  def log(line) do
    case System.get_env("WEAVE_MOCK_LOG") do
      nil -> :ok
      path -> File.write!(path, line <> "\n", [:append])
    end
  end

  def answer(load_ms) do
    IO.puts(
      ~s({"model":"fixture-64","entries":[) <>
        ~s({"entity":"mem:alpha","similarity":0.91,"label":"alpha hook"},) <>
        ~s({"entity":"note:orphan","similarity":0.42,"label":"orphan label"}],) <>
        ~s("expansion":null,"load_ms":#{load_ms},"embed_ms":1})
    )
  end

  def run(args) do
    # Both subcommands carry what the mock needs as the final argument:
    # `query` its Datalog program, `recall` its positional query.
    last = List.last(args)

    case Enum.find(args, &(&1 in ["recall", "query"])) do
      "query" ->
        rows =
          if String.contains?(last, "attr(") do
            ~s({"rows":[{"S":1,"E":"mem:alpha","A":"mem/desc","V":"alpha hook"},) <>
              ~s({"S":2,"E":"mem:alpha","A":"mem/type","V":"project"},) <>
              ~s({"S":3,"E":"mem:alpha","A":"mem/topic","V":"nix"}]})
          else
            ~s({"rows":[{"S":3,"T":1750000000000,"I":"blake3:aaaa",) <>
              ~s("E":"mem:alpha","D":"alpha hook"}]})
          end

        IO.puts(rows)

      "recall" ->
        log("spawn #{last}")
        answer(42)

        :stdio
        |> IO.stream(:line)
        |> Enum.each(fn line ->
          case String.trim_trailing(line, "\n") do
            "" ->
              :ok

            query ->
              log("line #{query}")
              answer("null")
          end
        end)

      _other ->
        IO.puts(:stderr, "mock weave: unhandled args: #{inspect(args)}")
        System.halt(64)
    end
  end
end

WeaveMock.run(System.argv())
