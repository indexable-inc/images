defmodule SymphonyElixir.DSL.InterpreterTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.DSL.Interpreter
  alias SymphonyElixir.DSL.Parser
  alias SymphonyElixir.IR.Node

  defp parse!(source) do
    {:ok, ast} = Parser.parse(source)
    ast
  end

  # Node carries wall-clock created_at/updated_at, which differ between
  # expand calls and are not part of the determinism contract. Compare the
  # structural fields that the interpreter actually decides.
  defp structural(nodes) when is_list(nodes), do: Enum.map(nodes, &structural/1)

  defp structural(%Node{} = node) do
    Map.take(node, [:id, :ast_origin, :kind, :envelope, :prompt_ref, :inputs, :deps, :expansion_key, :state, :output])
  end

  describe "expand/3 effect emission" do
    test "only effectful constructors become IR nodes; lets do not" do
      ast =
        parse!("""
        workflow "w" {
          label = "build-1"
          run <- agent { engine: codex, model: "m", prompt: inline "go" }
        }
        """)

      {delta, _pending, _log} = Interpreter.expand(ast, %{}, [])

      assert [%Node{kind: :agent, id: "agent-0"}] = delta
    end

    test "agent node carries the envelope spec map and prompt ref" do
      ast =
        parse!("""
        workflow "w" {
          run <- agent {
            engine: codex
            model: "m"
            permissions: read_only
            prompt: skill "inspect" { repo: "symphony" }
          }
        }
        """)

      {[node], _pending, _log} = Interpreter.expand(ast, %{}, [])

      assert node.envelope == %{"engine" => "codex", "model" => "m", "permissions" => "read_only"}
      assert {:skill, "inspect", %{"repo" => "symphony"}} = node.prompt_ref
      assert node.inputs["repo"] == {:literal, "symphony"}
    end
  end

  describe "derived deps and parallelism" do
    test "a downstream read of a binding becomes a node edge" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
          b <- agent { engine: codex, model: "m", prompt: skill "next" { ctx: ${a.area} } }
        }
        """)

      {delta, _pending, _log} = Interpreter.expand(ast, %{}, [])
      by_id = Map.new(delta, &{&1.id, &1})

      a = by_id["agent-0"]
      b = by_id["agent-1"]

      assert a.deps == []
      assert b.inputs["ctx"] == {:node, "agent-0", ["area"]}
      assert b.deps == ["agent-0"]
    end

    test "two data-independent binds have no edge and run in parallel" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "one" }
          b <- agent { engine: claude, model: "haiku", prompt: inline "two" }
        }
        """)

      {delta, _pending, _log} = Interpreter.expand(ast, %{}, [])

      assert Enum.all?(delta, &(&1.deps == []))
      assert delta |> Enum.map(& &1.id) |> Enum.sort() == ["agent-0", "agent-1"]
    end
  end

  describe "when gate" do
    test "emits a placeholder while the gating input is unresolved" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "do" }
          when ${a.changed} {
            n <- exec "./n.sh"
          }
        }
        """)

      {delta, pending, log} = Interpreter.expand(ast, %{}, [])
      by_kind = Enum.group_by(delta, & &1.kind)

      assert [gate] = by_kind[:gate]
      assert gate.inputs["gate"] == {:node, "agent-0", ["changed"]}
      assert gate.deps == ["agent-0"]
      # the body exec is not emitted yet
      assert by_kind[:exec] == nil
      assert {:awaiting, "when-1", ["agent-0"]} in pending
      assert log == []
    end

    test "expands the body only when the input resolves truthy" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "do" }
          when ${a.changed} {
            n <- exec "./n.sh"
          }
        }
        """)

      known = %{"agent-0" => %{"changed" => true}}
      {delta, _pending, log} = Interpreter.expand(ast, known, [])

      assert Enum.any?(delta, &(&1.kind == :exec))
      assert [%{observed: %{gate: :when, opened: true}}] = log
    end

    test "skips the body when the input resolves falsy" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "do" }
          when ${a.changed} {
            n <- exec "./n.sh"
          }
        }
        """)

      known = %{"agent-0" => %{"changed" => false}}
      {delta, _pending, log} = Interpreter.expand(ast, known, [])

      refute Enum.any?(delta, &(&1.kind == :exec))
      assert [%{observed: %{gate: :when, opened: false}}] = log
    end

    # An exec node's structured result (ExecRunner decodes SYMPHONY_OUTPUT_FILE
    # JSON into `output`) gates a downstream agent: the dig walks the
    # atom-keyed result map, then the string-keyed decoded JSON.
    test "opens on a truthy field of an exec node's structured output" do
      ast =
        parse!("""
        workflow "w" {
          gate <- exec "scripts/gate.sh" { title_prefix: "idiomatic:" }
          when ${gate.output.proceed} {
            n <- agent { engine: codex, model: "m", prompt: inline "go" }
          }
        }
        """)

      known = %{"exec-0" => %{kind: :exec, exit_code: 0, output: %{"proceed" => true}, log: ""}}
      {delta, _pending, log} = Interpreter.expand(ast, known, [])

      assert Enum.any?(delta, &(&1.kind == :agent))
      assert [%{observed: %{gate: :when, opened: true}}] = log
    end

    test "stays closed on a falsy field of an exec node's structured output" do
      ast =
        parse!("""
        workflow "w" {
          gate <- exec "scripts/gate.sh" { title_prefix: "idiomatic:" }
          when ${gate.output.proceed} {
            n <- agent { engine: codex, model: "m", prompt: inline "go" }
          }
        }
        """)

      known = %{"exec-0" => %{kind: :exec, exit_code: 0, output: %{"proceed" => false}, log: ""}}
      {delta, _pending, log} = Interpreter.expand(ast, known, [])

      refute Enum.any?(delta, &(&1.kind == :agent))
      assert [%{observed: %{gate: :when, opened: false}}] = log
    end
  end

  describe "every_nth gate" do
    # `every n` is one tick per run, evaluated at materialize against an
    # empty log. A run drives `expand_dynamic/1` several times (init, then
    # after each node success), re-feeding the grown log, so a re-pass must
    # reproduce the materialize decision and never advance the tick. The
    # cross-run tick advance is a separate concern (a future run would seed
    # its counter from the prior run's terminal log); the runtime today
    # never carries one run's log into the next run's materialize.
    test "evaluates one tick per run at the empty-log materialize pass" do
      ast =
        parse!("""
        workflow "w" {
          every 3 of gc {
            run <- exec "./gc.sh"
          }
        }
        """)

      # tick 1 (every 3): empty log -> skip.
      {d1, _p1, _log1} = Interpreter.expand(ast, %{}, [])
      refute Enum.any?(d1, &(&1.kind == :exec))

      one =
        parse!("""
        workflow "w" {
          every 1 of gc {
            run <- exec "./gc.sh"
          }
        }
        """)

      # tick 1 (every 1): fires immediately on the materialize pass.
      {d2, _p2, _log2} = Interpreter.expand(one, %{}, [])
      assert Enum.any?(d2, &(&1.kind == :exec))
    end

    test "re-expansion within a run reproduces the tick, never advancing it" do
      ast =
        parse!("""
        workflow "w" {
          every 2 of c {
            run <- exec "./x.sh"
          }
        }
        """)

      # The first (materialize) pass against an empty log skips (tick 1 of 2)
      # and records the decision in the log.
      {d0, _p0, log_after_skip} = Interpreter.expand(ast, %{}, [])
      refute Enum.any?(d0, &(&1.kind == :exec))

      # Re-feeding that log (what `expand_dynamic/1` does on every later
      # pass) reproduces the recorded skip rather than advancing to a fire,
      # so the live graph and a cold replay stay identical. This is the
      # replay invariant from `IR.RunGraph`.
      {a, _, log_a} = Interpreter.expand(ast, %{}, log_after_skip)
      {b, _, log_b} = Interpreter.expand(ast, %{}, log_after_skip)
      assert structural(a) == structural(b)
      refute Enum.any?(a, &(&1.kind == :exec))
      # No duplicate tick event is appended on a reproduction pass.
      assert length(log_a) == length(log_after_skip)
      assert log_a == log_b
    end

    test "a fired tick re-emits its body idempotently on re-expansion" do
      ast =
        parse!("""
        workflow "w" {
          every 1 of c {
            run <- exec "./x.sh"
          }
        }
        """)

      # tick 1 fires; the body exec is emitted and one fire event is logged.
      {d0, _p0, log0} = Interpreter.expand(ast, %{}, [])
      assert Enum.any?(d0, &(&1.kind == :exec))

      # A re-pass re-emits the same body (so the materializer re-derives and
      # merges it) without appending a second fire event.
      {d1, _p1, log1} = Interpreter.expand(ast, %{}, log0)
      assert Enum.any?(d1, &(&1.kind == :exec))
      assert structural(d0) == structural(d1)
      assert length(log1) == length(log0)
    end
  end

  describe "map fan-out" do
    test "emits one keyed child per element once the list resolves" do
      ast =
        parse!("""
        workflow "w" {
          seed <- agent { engine: codex, model: "m", prompt: inline "list repos" }
          map ${seed.repos} as repo {
            child <- exec "./audit.sh" { target: ${repo} }
          }
        }
        """)

      # unresolved: a single placeholder
      {d0, pending, _l0} = Interpreter.expand(ast, %{}, [])
      assert Enum.any?(d0, &(&1.kind == :map_fanout))
      assert {:awaiting, "map-1", ["agent-0"]} in pending

      # resolved: one exec per element, each binding the element literally
      known = %{"agent-0" => %{"repos" => ["alpha", "beta"]}}
      {d1, _p1, log} = Interpreter.expand(ast, known, [])

      execs = Enum.filter(d1, &(&1.kind == :exec))
      assert length(execs) == 2

      targets = execs |> Enum.map(& &1.inputs["target"]) |> Enum.sort()
      assert targets == [{:literal, "alpha"}, {:literal, "beta"}]

      assert [%{observed: %{gate: :map, count: 2}}] = log
      # children carry distinct ids derived from the fan-out key
      assert execs |> Enum.map(& &1.id) |> Enum.uniq() |> length() == 2
    end

    test "an empty list resolves to zero children and no placeholder" do
      ast =
        parse!("""
        workflow "w" {
          seed <- agent { engine: codex, model: "m", prompt: inline "list repos" }
          map ${seed.repos} as repo {
            child <- exec "./audit.sh" { target: ${repo} }
          }
        }
        """)

      known = %{"agent-0" => %{"repos" => []}}
      {delta, pending, log} = Interpreter.expand(ast, known, [])

      # No body child and no leftover placeholder: an empty fan-out emits
      # nothing for the materializer to schedule. The count event is still
      # logged so a replay reproduces the zero-child decision.
      refute Enum.any?(delta, &(&1.kind in [:exec, :map_fanout]))
      assert pending == []
      assert [%{observed: %{gate: :map, count: 0}}] = log
    end

    test "a non-list over folds to zero children rather than crashing" do
      ast =
        parse!("""
        workflow "w" {
          seed <- agent { engine: codex, model: "m", prompt: inline "scalar" }
          map ${seed.value} as it {
            child <- exec "./n.sh" { v: ${it} }
          }
        }
        """)

      # A scalar where a list is expected is a typed mismatch surfaced as an
      # empty fan-out, not an exception in the expand pass.
      known = %{"agent-0" => %{"value" => "not-a-list"}}
      {delta, _pending, log} = Interpreter.expand(ast, known, [])

      refute Enum.any?(delta, &(&1.kind in [:exec, :map_fanout]))
      assert [%{observed: %{gate: :map, over: :not_a_list}}] = log
    end

    test "re-expanding a fanned-out map re-emits identical children for an idempotent merge" do
      ast =
        parse!("""
        workflow "w" {
          seed <- agent { engine: codex, model: "m", prompt: inline "list repos" }
          map ${seed.repos} as repo {
            child <- exec "./audit.sh" { target: ${repo} }
          }
        }
        """)

      known = %{"agent-0" => %{"repos" => ["alpha", "beta"]}}

      # The fan-out is a pure function of the resolved list, so two passes
      # against the same known outputs emit byte-identical children. This is
      # what lets the materializer re-emit on every `expand_dynamic` pass and
      # merge by stable id without duplicating a child.
      {d1, p1, l1} = Interpreter.expand(ast, known, [])
      {d2, p2, l2} = Interpreter.expand(ast, known, [])
      assert structural(d1) == structural(d2)
      assert p1 == p2
      assert l1 == l2
    end
  end

  describe "determinism invariant" do
    test "expand is a pure function of its inputs" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "go" }
          when ${a.ok} {
            b <- exec "./b.sh"
          }
          map ${a.items} as it {
            c <- exec "./c.sh" { v: ${it} }
          }
        }
        """)

      known = %{"a" => nil, "agent-0" => %{"ok" => true, "items" => [1, 2, 3]}}

      {d1, p1, l1} = Interpreter.expand(ast, known, [])
      {d2, p2, l2} = Interpreter.expand(ast, known, [])

      assert structural(d1) == structural(d2)
      assert p1 == p2
      assert l1 == l2
    end
  end

  describe "bound gates" do
    test "a bound when gate binds the resolved body node so downstream reads it" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "do" }
          changed <- when ${a.flag} {
            n <- exec "./n.sh"
          }
          post <- agent { engine: codex, model: "m", prompt: skill "s" { from: ${changed.path} } }
        }
        """)

      known = %{"agent-0" => %{"flag" => true}}
      {delta, _pending, _log} = Interpreter.expand(ast, known, [])

      exec = Enum.find(delta, &(&1.kind == :exec))
      post = Enum.find(delta, &(&1.kind == :agent and &1.id != "agent-0"))

      assert exec, "the gate body exec should be emitted on the firing pass"
      # The gate's binding (`changed`) must point at the body node, not the
      # vanished placeholder, so the downstream edge resolves.
      assert post.inputs["from"] == {:node, exec.id, ["path"]}
      assert exec.id in post.deps
    end

    test "a bound every_nth gate binds the body node on the firing tick" do
      ast =
        parse!("""
        workflow "w" {
          tick <- every 1 of c {
            n <- exec "./n.sh"
          }
          post <- agent { engine: codex, model: "m", prompt: skill "s" { from: ${tick.path} } }
        }
        """)

      {delta, _pending, _log} = Interpreter.expand(ast, %{}, [])

      exec = Enum.find(delta, &(&1.kind == :exec))
      post = Enum.find(delta, &(&1.kind == :agent))

      assert exec
      assert post.inputs["from"] == {:node, exec.id, ["path"]}
      assert exec.id in post.deps
    end

    test "the gate placeholder is gone once the when input resolves" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "do" }
          when ${a.changed} {
            n <- exec "./n.sh"
          }
        }
        """)

      known = %{"agent-0" => %{"changed" => true}}
      {delta, _pending, _log} = Interpreter.expand(ast, known, [])

      refute Enum.any?(delta, &(&1.kind == :gate))
      assert Enum.any?(delta, &(&1.kind == :exec))
    end
  end

  describe "deferred inline prompts" do
    test "an inline prompt over an unresolved output defers, then folds to text" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
          b <- agent { engine: codex, model: "m", prompt: inline "use ${a.result} now" }
        }
        """)

      {d0, pending, _l0} = Interpreter.expand(ast, %{}, [])
      b0 = Enum.find(d0, &(&1.id == "agent-1"))
      assert b0.prompt_ref == {:inline, nil}
      assert {:awaiting, "agent-1", ["agent-0"]} in pending

      known = %{"agent-0" => %{"result" => "X"}}
      {d1, _p1, _l1} = Interpreter.expand(ast, known, [])
      b1 = Enum.find(d1, &(&1.id == "agent-1"))
      assert b1.prompt_ref == {:inline, "use X now"}
    end
  end
end
