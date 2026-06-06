defmodule SymphonyElixir.DSL.ParserTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.DSL.Parser

  @fixture Path.join(__DIR__, "fixtures/release.sym")

  describe "parse/1" do
    test "parses the release fixture into a workflow AST" do
      source = File.read!(@fixture)
      assert {:ok, ast} = Parser.parse(source)

      assert ast.kind == :workflow
      assert ast.name == "release"
      # three agent binds plus one when gate
      assert length(ast.statements) == 4
    end

    test "binds introduce names and effects carry envelope and prompt" do
      source = """
      workflow "one" {
        session <- agent {
          engine: codex
          model: "gpt-5.3-codex"
          permissions: workspace_write
          prompt: skill "inspect" { repo: "symphony" }
        }
      }
      """

      assert {:ok, ast} = Parser.parse(source)
      assert [{:bind, "session", agent}] = ast.statements
      assert agent.kind == :agent
      assert agent.envelope == %{"engine" => "codex", "model" => "gpt-5.3-codex", "permissions" => "workspace_write"}
      assert {:skill, "inspect", %{"repo" => {:literal, "symphony"}}} = agent.prompt
    end

    test "inline prompt interpolation lowers to a field read over a binding" do
      source = """
      workflow "two" {
        a <- agent { engine: codex, model: "m", prompt: inline "x" }
        b <- agent { engine: codex, model: "m", prompt: inline "use ${a.area} now" }
      }
      """

      assert {:ok, ast} = Parser.parse(source)
      assert [_a, {:bind, "b", agent_b}] = ast.statements
      assert {:inline, {:concat, parts}} = agent_b.prompt
      assert {:literal, "use "} = Enum.at(parts, 0)
      assert {:field, {:var, "a"}, ["area"]} = Enum.at(parts, 1)
      assert {:literal, " now"} = Enum.at(parts, 2)
    end

    test "every and map and exec parse with their combinator shape" do
      source = """
      workflow "combos" {
        every 3 of gc_counter {
          gc <- exec "./gc.sh" timeout 60
        }

        map ${seed.repos} as repo {
          child <- subrun "audit.sym" { target: ${repo} }
        }
      }
      """

      assert {:ok, ast} = Parser.parse(source)
      assert [every, map] = ast.statements
      assert every.kind == :every_nth
      assert every.n == 3
      assert every.counter == "gc_counter"
      assert {:bind, "gc", %{kind: :exec, timeout: {:literal, 60}}} = every.body

      assert map.kind == :map
      assert map.as == "repo"
      assert {:field, {:var, "seed"}, ["repos"]} = map.over
      assert {:bind, "child", %{kind: :subrun}} = map.body
    end

    test "diagnostics carry a 1-based line and column" do
      source = """
      workflow "bad" {
        x <- agent {
          engine: codex
          model: "m"
        }
        oops
      }
      """

      assert {:error, diag} = Parser.parse(source)
      assert is_binary(diag.message)
      assert is_integer(diag.line) and diag.line >= 1
      assert is_integer(diag.column) and diag.column >= 1
    end

    test "the diagnostic carries the file name a caller passes" do
      source = ~s(workflow "bad" { oops })

      assert {:error, diag} = Parser.parse(source, file: "bad.sym")
      assert diag.file == "bad.sym"

      # An anonymous string parse has no file.
      assert {:error, anon} = Parser.parse(source)
      assert anon.file == nil
    end

    test "a tokenizer error also carries the caller's file name" do
      # The unterminated string fails in the lexer, before a parse state
      # exists; the file still lands on the diagnostic.
      source = ~s(workflow "u" {\n  x <- agent { engine: codex, model: "oops\n}\n)

      assert {:error, diag} = Parser.parse(source, file: "u.sym")
      assert diag.file == "u.sym"
      assert diag.message =~ "string"
    end

    test "a missing prompt is a load error" do
      source = """
      workflow "np" {
        x <- agent { engine: codex, model: "m" }
      }
      """

      assert {:error, diag} = Parser.parse(source)
      assert diag.message =~ "prompt"
    end

    test "an unterminated string reports the open position" do
      source = ~s(workflow "u" {\n  x <- agent { engine: codex, model: "oops\n}\n)
      assert {:error, diag} = Parser.parse(source)
      assert diag.message =~ "string"
      assert diag.line == 2
    end
  end

  describe "trigger header" do
    defp parse!(source) do
      {:ok, ast} = Parser.parse(source)
      ast
    end

    test "a workflow with no `on` clause has a nil trigger" do
      assert parse!(~s(workflow "w" { a <- agent { engine: codex, model: "m", prompt: inline "go" } })).trigger ==
               nil
    end

    test "manual" do
      assert parse!(~s(workflow "w" on manual { a <- exec "./x.sh" })).trigger == %{kind: :manual}
    end

    test "linear normalizes the label" do
      assert parse!(~s(workflow "w" on linear label "[Sym] Implement" { a <- exec "./x.sh" })).trigger ==
               %{kind: :linear, label: "[sym] implement"}
    end

    test "cron carries schedule, timezone, and input" do
      source = ~s|workflow "w" on cron "0 9 * * *" tz "UTC" input { lookback_hours: 5 } { a <- exec "./x.sh" }|

      assert parse!(source).trigger == %{
               kind: :cron,
               schedule: "0 9 * * *",
               timezone: "UTC",
               input: %{"lookback_hours" => 5}
             }
    end

    test "cron defaults the timezone and input when omitted" do
      assert parse!(~s|workflow "w" on cron "* * * * *" { a <- exec "./x.sh" }|).trigger == %{
               kind: :cron,
               schedule: "* * * * *",
               timezone: "UTC",
               input: %{}
             }
    end

    test "slack_huddle and slack_mention map to the runtime kinds" do
      assert parse!(~s(workflow "w" on slack_huddle channel "focus" { a <- exec "./x.sh" })).trigger ==
               %{kind: :slack_huddle_completed, channel: "focus"}

      assert parse!(~s(workflow "w" on slack_mention channel "#playbook" { a <- exec "./x.sh" })).trigger ==
               %{kind: :slack_app_mention, channel: "#playbook"}
    end

    test "github_pr_label carries repo and normalized label" do
      assert parse!(~s(workflow "w" on github_pr_label repo "indexable-inc/ix" label "Review-Loop" { a <- exec "./x.sh" })).trigger ==
               %{kind: :github_pr_label, repo: "indexable-inc/ix", label: "review-loop"}
    end

    test "an unknown trigger kind is a diagnostic" do
      assert {:error, diag} = Parser.parse(~s(workflow "w" on telepathy { a <- exec "./x.sh" }))
      assert diag.message =~ "trigger kind"
    end
  end
end
