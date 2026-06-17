defmodule SymphonyElixir.DSL.AST do
  @moduledoc """
  Reified data constructors for the workflow surface language.

  The DSL has three representations (see the overhaul plan, Pillar 1):

      .sym source --parse--> reified AST --interpret/expand--> IR graph

  This module is the middle one. Every combinator is plain data, never a
  host closure. A closure could not be serialized into the durable
  `RunGraph`, inspected in the dashboard, or replayed deterministically
  after a restart, so the whole surface is reified as structs the
  interpreter walks.

  ## Grammar

  A workflow is a `do`-block: an ordered list of statements. The block is
  monadic in the sense that a binding introduces a name that later
  statements read, and reading a name is what creates a data dependency.
  Two statements whose inputs do not reference each other have no edge and
  the interpreter is free to schedule them in parallel.

      workflow   := do { stmt* }
      stmt       := bind | let | expr
      bind       := name "<-" expr          # name binds the effect's output
      let        := name "="  pure          # name binds a pure value
      expr       := effect | pure

  ### Effects (become IR nodes)

  Only effectful constructors materialize as `IR.Node`s. They are the
  things that talk to the world: an engine turn, a shell script, a child
  run.

      agent  envelope: <map>, prompt: <prompt_ref>, inputs: %{name => pure}
      exec   script: <pure>, inputs: %{name => pure}, timeout: <pure>?
      subrun source: <pure>, inputs: %{name => pure}

  An `agent` carries an `Engine.Envelope` spec as a plain map (validated
  downstream by `Engine.Envelope.from_map/1`) plus a `prompt_ref`:

      prompt_ref := {:skill, name, bindings} | {:inline, text}

  ### Higher-order combinators (dynamic expansion)

  These take a gating input and a body. When the gate's input is
  unresolved the interpreter emits a placeholder node (`:gate` or
  `:map_fanout`); when the input resolves it re-runs `expand` and emits
  the body deterministically.

      when     cond: <pure-over-bindings>, body: <expr>
      every_nth n: <pos_integer>, key: <counter-name>, body: <expr>
      map      over: <pure-list>, as: <name>, body: <expr>

  `when` runs its body only if `cond` is truthy. `every_nth` runs its body
  on every nth expansion of a persisted, named counter (a pure function of
  that counter, so replay is deterministic: no wall clock, no RNG).
  `map` fans the body out once per element of `over`, binding each element
  to `as` inside the body.

  ### Pure expressions (evaluated at expand time, never become nodes)

      pure := literal | var | field | concat | list
      literal := string | integer | float | boolean | nil
      var     := {:var, name}                 # read a binding
      field   := {:field, pure, path}         # e.g. `area session` -> nested read
      concat  := {:concat, [pure]}            # string interpolation / joining
      list    := {:list, [pure]}

  Pure expressions are computed inside the interpreter. `{:field, {:var,
  "session"}, ["area"]}` reads `known_outputs["session"]["area"]` at
  expand time; it never becomes a trivial IR node.

  ## Stable AST ids

  Every constructor carries an `id` so the interpreter can derive a stable
  IR node id and the runtime can record which AST construct an expansion
  came from. The parser assigns ids positionally; `with_id/2` is the
  single writer so the scheme stays in one place.
  """

  @typedoc "A bind introduces `name` from an effectful expression's output."
  @type bind :: {:bind, String.t(), expr()}

  @typedoc "A let introduces `name` from a pure value computed at expand time."
  @type let :: {:let, String.t(), pure()}

  @typedoc """
  The declaration that fires a workflow, parsed from the `on <trigger>`
  header clause. The normalized shape matches the runtime's trigger maps so
  the catalog can index a workflow by `kind` and a producer can match an
  event against it. `nil` when the header omits `on` (a workflow only an
  operator starts by name).
  """
  @type trigger :: %{required(:kind) => atom(), optional(atom()) => term()} | nil

  @typedoc "A workflow is an ordered list of statements in a do-block, with an optional trigger."
  @type workflow :: %{
          kind: :workflow,
          id: String.t(),
          name: String.t() | nil,
          trigger: trigger(),
          statements: [statement()]
        }

  @type statement :: bind() | let() | expr()

  @typedoc "Effectful constructors materialize as IR nodes; pure ones do not."
  @type expr :: effect() | pure()

  @type effect ::
          agent()
          | exec()
          | subrun()
          | when_()
          | every_nth()
          | map_()

  @type agent :: %{
          kind: :agent,
          id: String.t(),
          envelope: map(),
          prompt: prompt_ref(),
          inputs: %{optional(String.t()) => pure()}
        }

  @type exec :: %{
          kind: :exec,
          id: String.t(),
          script: pure(),
          timeout: pure() | nil,
          inputs: %{optional(String.t()) => pure()}
        }

  @type subrun :: %{
          kind: :subrun,
          id: String.t(),
          source: pure(),
          inputs: %{optional(String.t()) => pure()}
        }

  @type when_ :: %{
          kind: :when,
          id: String.t(),
          cond: pure(),
          body: expr()
        }

  @type every_nth :: %{
          kind: :every_nth,
          id: String.t(),
          n: pos_integer(),
          counter: String.t(),
          body: expr()
        }

  @type map_ :: %{
          kind: :map,
          id: String.t(),
          over: pure(),
          as: String.t(),
          body: expr()
        }

  @type prompt_ref ::
          {:skill, String.t(), %{optional(String.t()) => pure()}}
          | {:inline, pure()}

  @type pure ::
          {:literal, term()}
          | {:var, String.t()}
          | {:field, pure(), [String.t()]}
          | {:concat, [pure()]}
          | {:list, [pure()]}

  @effect_kinds [:agent, :exec, :subrun, :when, :every_nth, :map]

  @doc "The effectful constructor kinds. Only these become IR nodes."
  @spec effect_kinds() :: [atom()]
  def effect_kinds, do: @effect_kinds

  @doc "True when an AST expression is an effectful constructor (emits a node)."
  @spec effect?(term()) :: boolean()
  def effect?(%{kind: kind}) when kind in @effect_kinds, do: true
  # astlog-ignore: public-def-needs-spec
  def effect?(_), do: false

  @doc "True when an AST expression is a pure value (evaluated, never a node)."
  @spec pure?(term()) :: boolean()
  def pure?({tag, _}) when tag in [:literal, :var], do: true
  # astlog-ignore: public-def-needs-spec
  def pure?({tag, _, _}) when tag in [:field], do: true
  # astlog-ignore: public-def-needs-spec
  def pure?({tag, list}) when tag in [:concat, :list] and is_list(list), do: true
  # astlog-ignore: public-def-needs-spec
  def pure?(_), do: false

  # --- constructors -------------------------------------------------------

  @doc "Build a workflow do-block from an ordered statement list and an optional trigger."
  @spec workflow(String.t() | nil, trigger(), [statement()], String.t()) :: workflow()
  def workflow(name, trigger, statements, id)
      when is_list(statements) and is_binary(id) and (is_nil(trigger) or is_map(trigger)) do
    %{kind: :workflow, id: id, name: name, trigger: trigger, statements: statements}
  end

  @doc "A `name <- effect` binding."
  @spec bind(String.t(), expr()) :: bind()
  def bind(name, expr) when is_binary(name), do: {:bind, name, expr}

  @doc "A `name = pure` binding."
  @spec let(String.t(), pure()) :: let()
  def let(name, pure) when is_binary(name), do: {:let, name, pure}

  @doc "An agent-call node carrying an envelope spec map and a prompt ref."
  @spec agent(map(), prompt_ref(), %{optional(String.t()) => pure()}, String.t()) :: agent()
  def agent(envelope, prompt, inputs, id)
      when is_map(envelope) and is_map(inputs) and is_binary(id) do
    %{kind: :agent, id: id, envelope: envelope, prompt: prompt, inputs: inputs}
  end

  @doc "An exec (shell script) node."
  @spec exec(pure(), pure() | nil, %{optional(String.t()) => pure()}, String.t()) :: exec()
  def exec(script, timeout, inputs, id) when is_map(inputs) and is_binary(id) do
    %{kind: :exec, id: id, script: script, timeout: timeout, inputs: inputs}
  end

  @doc "A subrun (first-class child run) node."
  @spec subrun(pure(), %{optional(String.t()) => pure()}, String.t()) :: subrun()
  def subrun(source, inputs, id) when is_map(inputs) and is_binary(id) do
    %{kind: :subrun, id: id, source: source, inputs: inputs}
  end

  @doc "A `when cond do body` conditional combinator."
  @spec when_(pure(), expr(), String.t()) :: when_()
  def when_(cond, body, id) when is_binary(id) do
    %{kind: :when, id: id, cond: cond, body: body}
  end

  @doc "An `every_nth n counter do body` gate keyed on a persisted counter."
  @spec every_nth(pos_integer(), String.t(), expr(), String.t()) :: every_nth()
  def every_nth(n, counter, body, id)
      when is_integer(n) and n > 0 and is_binary(counter) and is_binary(id) do
    %{kind: :every_nth, id: id, n: n, counter: counter, body: body}
  end

  @doc "A `map over as elem do body` fan-out combinator."
  @spec map_(pure(), String.t(), expr(), String.t()) :: map_()
  def map_(over, as, body, id) when is_binary(as) and is_binary(id) do
    %{kind: :map, id: id, over: over, as: as, body: body}
  end

  # --- pure value constructors -------------------------------------------

  @spec literal(term()) :: pure()
  def literal(value), do: {:literal, value}

  @spec var(String.t()) :: pure()
  def var(name) when is_binary(name), do: {:var, name}

  @spec field(pure(), [String.t()]) :: pure()
  def field(base, path) when is_list(path), do: {:field, base, path}

  @spec concat([pure()]) :: pure()
  def concat(parts) when is_list(parts), do: {:concat, parts}

  @spec list([pure()]) :: pure()
  def list(items) when is_list(items), do: {:list, items}

  @doc """
  Attach an id to a constructor map. The single writer of the id scheme so
  the parser and any future builder cannot drift apart.
  """
  @spec with_id(map(), String.t()) :: map()
  def with_id(node, id) when is_map(node) and is_binary(id), do: Map.put(node, :id, id)
end
