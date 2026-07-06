defmodule SymphonyElixir.DSL.Parser do
  @moduledoc """
  Parse a standalone `.sym` workflow surface into a reified
  `SymphonyElixir.DSL.AST`.

  `parse/1` returns `{:ok, ast}` or `{:error, diagnostic}` where the
  diagnostic carries a source span (line and column) so an operator can
  jump straight to the offending token. The surface is monadic in spirit:
  a `name <- effect` binding introduces a data dependency, and statements
  whose inputs do not reference each other are independent (the
  interpreter is then free to run them in parallel).

  ## Surface syntax

      workflow "release" {
        session <- agent {
          engine: codex
          model: "gpt-5.3-codex"
          permissions: workspace_write
          location: local
          prompt: skill "inspect" { repo: "symphony" }
        }

        summary <- agent {
          engine: claude
          model: haiku
          prompt: inline "summarize ${session.report}"
        }

        when ${summary.ok} {
          notice <- exec "./scripts/notify.sh" timeout 30
        }

        every 3 of build_counter {
          gc <- exec "./scripts/gc.sh"
        }

        map ${session.repos} as repo {
          child <- subrun "audit.sym" { target: ${repo} }
        }
      }

  A statement is `name <- <effect>`, `name = <pure>`, or a bare effect.
  An effect is `agent { ... }`, `exec <string> [timeout <int>] [{ ... }]`,
  `subrun <string> [{ ... }]`, `when <pure> { stmt* }`,
  `every <int> of <counter> { stmt* }`, or
  `map <pure> as <name> { stmt* }`.

  Pure values are string literals (`"..."`, with `${path}` interpolation),
  integers, floats, `true`/`false`/`null`, bracketed lists `[a, b]`, and
  interpolation references `${name.field.path}`.

  ## Diagnostics

  Every diagnostic is `{:error, %{message: String.t(), line: pos_integer,
  column: pos_integer, file: String.t() | nil, got: term()}}`. Line and
  column are 1-based and point at the token that failed, recovered from the
  tokenizer's per-token span so the message lands on the real source
  position rather than a byte offset. `file` is the source filename a caller
  passed through `parse/2`; it is `nil` for an anonymous string parse and is
  filled in by the `WorkflowCatalog` so an author sees which `.sym` broke.
  """

  alias SymphonyElixir.DSL.AST
  alias SymphonyElixir.DSL.Parser.Lexer
  alias SymphonyElixir.Runtime.Trigger

  @type diagnostic :: %{
          message: String.t(),
          line: pos_integer(),
          column: pos_integer(),
          file: String.t() | nil,
          got: term()
        }

  # The surface keywords `parse_trigger_kind/2` dispatches on. This list is
  # the source of truth for the `on <kind>` vocabulary so the schema
  # endpoint and the run form offer exactly the kinds the parser accepts;
  # adding a kind here plus its dispatch clause flows to the UI without a
  # form edit.
  @trigger_kinds ~w(manual cron linear slack_huddle slack_mention github_pr_label)a

  @doc "The trigger kinds the `on` clause accepts, as surface keywords."
  @spec trigger_kinds() :: [atom()]
  def trigger_kinds, do: @trigger_kinds

  @doc """
  Parse `.sym` source. `opts` accepts `:file`, the source filename the
  catalog stamps onto a diagnostic so an author sees which `.sym` broke; it
  defaults to `nil` for an anonymous string parse.
  """
  @spec parse(String.t(), keyword()) :: {:ok, AST.workflow()} | {:error, diagnostic()}
  def parse(source, opts \\ []) when is_binary(source) and is_list(opts) do
    file = Keyword.get(opts, :file)

    case Lexer.tokenize(source) do
      {:ok, tokens} ->
        state = %{tokens: tokens, counter: 0, file: file}

        case parse_workflow(state) do
          {:ok, ast, rest} ->
            case skip_to_end(rest) do
              :ok -> {:ok, ast}
              {:error, _} = err -> err
            end

          {:error, _} = err ->
            err
        end

      # The lexer fails before a parse state exists, so it cannot know the
      # file. Stamp it here so a tokenizer error (unterminated string, stray
      # character) lands on the same located shape as a parser error.
      {:error, diag} ->
        {:error, Map.put(diag, :file, file)}
    end
  end

  # --- workflow / statements ---------------------------------------------

  # The workflow keeps a fixed id outside the effect counter so the first
  # effect is `agent-0`; only effect constructors consume positional ids.
  defp parse_workflow(state) do
    with {:ok, _, s1} <- expect(state, :keyword, "workflow"),
         {:ok, name_tok, s2} <- optional_string(s1),
         {:ok, trigger, s3} <- parse_optional_trigger(s2),
         {:ok, _, s4} <- expect(s3, :lbrace, "{"),
         {:ok, statements, s5} <- parse_statements(s4, []),
         {:ok, _, s6} <- expect(s5, :rbrace, "}") do
      name = if name_tok, do: name_tok.value
      {:ok, AST.workflow(name, trigger, statements, "workflow"), s6}
    end
  end

  # --- trigger header -----------------------------------------------------

  # `on <kind> <params>` declares what fires the workflow. The header is
  # optional; a workflow with no `on` clause has a nil trigger and is only
  # started by an operator. The keyword markers (`on`, the kind, the param
  # names) are plain identifiers, so the lexer needs no trigger vocabulary.
  # The normalized maps match the runtime's trigger shapes so the catalog
  # and producers reuse one representation.
  defp parse_optional_trigger(state) do
    case peek(state) do
      %{type: :ident, value: "on"} -> parse_trigger(advance(state))
      _ -> {:ok, nil, state}
    end
  end

  defp parse_trigger(state) do
    case peek(state) do
      %{type: :ident, value: kind} -> parse_trigger_kind(kind, advance(state))
      other -> error(state, "expected a trigger kind after `on`", token_value(other))
    end
  end

  defp parse_trigger_kind("manual", state), do: {:ok, %{kind: :manual}, state}

  defp parse_trigger_kind("cron", state) do
    with {:ok, schedule, s1} <- trigger_string(state, "cron schedule") do
      {timezone, s2} = optional_labeled_string(s1, "tz", "UTC")
      {input, s3} = parse_optional_trigger_input(s2)
      {:ok, %{kind: :cron, schedule: String.trim(schedule), timezone: timezone, input: input}, s3}
    end
  end

  defp parse_trigger_kind("linear", state) do
    with {:ok, label, s1} <- labeled_string(state, "label") do
      {:ok, %{kind: :linear, label: Trigger.normalize_label(label)}, s1}
    end
  end

  defp parse_trigger_kind("slack_huddle", state) do
    with {:ok, channel, s1} <- labeled_string(state, "channel") do
      {:ok, %{kind: :slack_huddle_completed, channel: String.trim(channel)}, s1}
    end
  end

  defp parse_trigger_kind("slack_mention", state) do
    with {:ok, channel, s1} <- labeled_string(state, "channel") do
      {:ok, %{kind: :slack_app_mention, channel: String.trim(channel)}, s1}
    end
  end

  defp parse_trigger_kind("github_pr_label", state) do
    with {:ok, repo, s1} <- labeled_string(state, "repo"),
         {:ok, label, s2} <- labeled_string(s1, "label") do
      {:ok, %{kind: :github_pr_label, repo: String.trim(repo), label: Trigger.normalize_label(label)}, s2}
    end
  end

  defp parse_trigger_kind(other, state), do: error(state, "unknown trigger kind #{inspect(other)}", other)

  # `<name> "<value>"`: a labeled string param such as `label "..."` or
  # `repo "..."`. The label is a bare identifier; the value is a string.
  defp labeled_string(state, label) do
    with {:ok, _, s1} <- expect_ident_value(state, label) do
      trigger_string(s1, label)
    end
  end

  defp optional_labeled_string(state, label, default) do
    case peek(state) do
      %{type: :ident, value: ^label} ->
        case trigger_string(advance(state), label) do
          {:ok, value, rest} -> {String.trim(value), rest}
          {:error, _} -> {default, state}
        end

      _ ->
        {default, state}
    end
  end

  defp parse_optional_trigger_input(state) do
    case peek(state) do
      %{type: :ident, value: "input"} ->
        case parse_inputs_block(advance(state)) do
          {:ok, pairs, rest} -> {literal_input_map(pairs), rest}
          {:error, _} -> {%{}, state}
        end

      _ ->
        {%{}, state}
    end
  end

  # The cron input block is authored as literal pure values; collapse them
  # to plain data so the trigger map round-trips through JSON like the
  # pre-overhaul DAG cron input did.
  defp literal_input_map(pairs) do
    Map.new(pairs, fn
      {k, {:literal, v}} -> {k, v}
      {k, other} -> {k, other}
    end)
  end

  defp expect_ident_value(state, value) do
    case peek(state) do
      %{type: :ident, value: ^value} = tok -> {:ok, tok, advance(state)}
      other -> error(state, "expected #{inspect(value)}", token_value(other))
    end
  end

  defp trigger_string(state, what) do
    case peek(state) do
      %{type: :string} = tok -> {:ok, flatten_string(tok.value), advance(state)}
      other -> error(state, "expected a string for #{what}", token_value(other))
    end
  end

  defp parse_statements(state, acc) do
    case peek(state) do
      %{type: :rbrace} ->
        {:ok, Enum.reverse(acc), state}

      :eof ->
        {:ok, Enum.reverse(acc), state}

      _ ->
        with {:ok, stmt, rest} <- parse_statement(state) do
          parse_statements(rest, [stmt | acc])
        end
    end
  end

  # `name <- effect`, `name = pure`, or a bare effect.
  defp parse_statement(state) do
    case peek(state) do
      %{type: :ident} = ident ->
        rest = advance(state)

        case peek(rest) do
          %{type: :larrow} ->
            with {:ok, expr, s} <- parse_expr(advance(rest)) do
              {:ok, AST.bind(ident.value, expr), s}
            end

          %{type: :equals} ->
            with {:ok, pure, s} <- parse_pure(advance(rest)) do
              {:ok, AST.let(ident.value, pure), s}
            end

          _ ->
            # A leading identifier that is not a binding must be a bare
            # effect keyword (when/every/map). Re-dispatch on the keyword.
            parse_expr(state)
        end

      _ ->
        parse_expr(state)
    end
  end

  # --- expressions (effects and pures) -----------------------------------

  @effect_keywords ~w(agent exec subrun when every map)

  defp parse_expr(state) do
    case peek(state) do
      %{type: :keyword, value: kw} when kw in @effect_keywords ->
        parse_effect(kw, state)

      %{type: :keyword, value: kw} ->
        error(state, "unexpected keyword #{inspect(kw)} in expression position", kw)

      :eof ->
        error(state, "unexpected end of input where an expression was expected", :eof)

      other ->
        error(state, "expected an effect (agent/exec/subrun/when/every/map)", other.value)
    end
  end

  defp parse_effect("agent", state), do: parse_agent(state)
  defp parse_effect("exec", state), do: parse_exec(state)
  defp parse_effect("subrun", state), do: parse_subrun(state)
  defp parse_effect("when", state), do: parse_when(state)
  defp parse_effect("every", state), do: parse_every(state)
  defp parse_effect("map", state), do: parse_map(state)

  # Each effect reserves its id before parsing its body, so ids read in
  # source pre-order (a gate's id precedes its child's). The id is stable
  # under a re-parse of identical source, which is what the IR layer needs
  # to rebuild the same node ids on replay.
  defp parse_agent(state) do
    {id, state} = next_id(state, "agent")

    with {:ok, _, s1} <- expect(state, :keyword, "agent"),
         {:ok, _, s2} <- expect(s1, :lbrace, "{"),
         {:ok, fields, s3} <- parse_agent_fields(s2, %{prompt: nil, inputs: %{}, envelope: %{}}),
         {:ok, _, s4} <- expect(s3, :rbrace, "}"),
         {:ok, prompt} <- require_prompt(fields, state) do
      {:ok, AST.agent(fields.envelope, prompt, fields.inputs, id), s4}
    end
  end

  @envelope_keys ~w(engine model effort permissions location)

  # Agent fields may be separated by newlines or commas; a stray leading
  # comma between fields is skipped so both layouts parse the same.
  defp parse_agent_fields(%{tokens: [%{type: :comma} | _]} = state, acc) do
    parse_agent_fields(advance(state), acc)
  end

  defp parse_agent_fields(state, acc) do
    case peek(state) do
      %{type: :rbrace} ->
        {:ok, acc, state}

      %{type: :ident, value: "prompt"} ->
        with {:ok, _, s1} <- expect(state, :ident, "prompt"),
             {:ok, _, s2} <- expect(s1, :colon, ":"),
             {:ok, prompt, s3} <- parse_prompt(s2) do
          parse_agent_fields(s3, %{acc | prompt: prompt})
        end

      %{type: :ident, value: "inputs"} ->
        with {:ok, _, s1} <- expect(state, :ident, "inputs"),
             {:ok, _, s2} <- expect(s1, :colon, ":"),
             {:ok, inputs, s3} <- parse_inputs_block(s2) do
          parse_agent_fields(s3, %{acc | inputs: inputs})
        end

      %{type: :ident, value: key} when key in @envelope_keys ->
        with {:ok, _, s1} <- expect(state, :ident, key),
             {:ok, _, s2} <- expect(s1, :colon, ":"),
             {:ok, value, s3} <- parse_envelope_value(s2, key) do
          parse_agent_fields(s3, %{acc | envelope: Map.put(acc.envelope, key, value)})
        end

      %{type: :ident, value: other} ->
        error(state, "unknown agent field #{inspect(other)}", other)

      other ->
        error(state, "expected an agent field name", token_value(other))
    end
  end

  # Envelope scalars are bare identifiers (engine: codex) or strings
  # (model: "gpt-5.3-codex"). They are kept as plain values for
  # `Engine.Envelope.from_map/1` to validate downstream.
  defp parse_envelope_value(state, _key) do
    case peek(state) do
      %{type: :ident, value: v} -> {:ok, v, advance(state)}
      %{type: :keyword, value: v} -> {:ok, v, advance(state)}
      %{type: :string, value: v} -> {:ok, flatten_string(v), advance(state)}
      other -> error(state, "expected an envelope value", token_value(other))
    end
  end

  defp parse_prompt(state) do
    case peek(state) do
      %{type: :keyword, value: "skill"} ->
        with {:ok, _, s1} <- expect(state, :keyword, "skill"),
             {:ok, name_tok, s2} <- expect_string(s1) do
          parse_skill_bindings(s2, name_tok.value)
        end

      %{type: :keyword, value: "inline"} ->
        with {:ok, _, s1} <- expect(state, :keyword, "inline"),
             {:ok, pure, s2} <- parse_pure(s1) do
          {:ok, {:inline, pure}, s2}
        end

      other ->
        error(state, ~s{expected a prompt (skill "name" or inline "text")}, token_value(other))
    end
  end

  defp parse_skill_bindings(state, name) do
    case peek(state) do
      %{type: :lbrace} ->
        with {:ok, bindings, next} <- parse_inputs_block(state) do
          {:ok, {:skill, name, bindings}, next}
        end

      _ ->
        {:ok, {:skill, name, %{}}, state}
    end
  end

  defp parse_exec(state) do
    {id, state} = next_id(state, "exec")

    with {:ok, _, s1} <- expect(state, :keyword, "exec"),
         {:ok, script, s2} <- parse_pure(s1) do
      {timeout, s3} = parse_optional_timeout(s2)

      with {:ok, inputs, s4} <- parse_optional_inputs(s3) do
        {:ok, AST.exec(script, timeout, inputs, id), s4}
      end
    end
  end

  defp parse_optional_timeout(state) do
    case peek(state) do
      %{type: :keyword, value: "timeout"} ->
        rest = advance(state)

        case peek(rest) do
          %{type: :int, value: n} -> {AST.literal(n), advance(rest)}
          _ -> {nil, state}
        end

      _ ->
        {nil, state}
    end
  end

  defp parse_subrun(state) do
    {id, state} = next_id(state, "subrun")

    with {:ok, _, s1} <- expect(state, :keyword, "subrun"),
         {:ok, source, s2} <- parse_pure(s1),
         {:ok, inputs, s3} <- parse_optional_inputs(s2) do
      {:ok, AST.subrun(source, inputs, id), s3}
    end
  end

  defp parse_when(state) do
    {id, state} = next_id(state, "when")

    with {:ok, _, s1} <- expect(state, :keyword, "when"),
         {:ok, cond, s2} <- parse_pure(s1),
         {:ok, body, s3} <- parse_block_single(s2) do
      {:ok, AST.when_(cond, body, id), s3}
    end
  end

  defp parse_every(state) do
    {id, state} = next_id(state, "every")

    with {:ok, _, s1} <- expect(state, :keyword, "every"),
         {:ok, n_tok, s2} <- expect_int(s1),
         {:ok, _, s3} <- expect(s2, :keyword, "of"),
         {:ok, counter_tok, s4} <- expect_ident(s3),
         {:ok, body, s5} <- parse_block_single(s4) do
      {:ok, AST.every_nth(n_tok.value, counter_tok.value, body, id), s5}
    end
  end

  defp parse_map(state) do
    {id, state} = next_id(state, "map")

    with {:ok, _, s1} <- expect(state, :keyword, "map"),
         {:ok, over, s2} <- parse_pure(s1),
         {:ok, _, s3} <- expect(s2, :keyword, "as"),
         {:ok, as_tok, s4} <- expect_ident(s3),
         {:ok, body, s5} <- parse_block_single(s4) do
      {:ok, AST.map_(over, as_tok.value, body, id), s5}
    end
  end

  # A combinator body is a brace block with exactly one statement. One
  # statement keeps the gate's emitted child unambiguous; nesting another
  # do-block is how a body grows past one effect.
  defp parse_block_single(state) do
    with {:ok, _, s1} <- expect(state, :lbrace, "{"),
         {:ok, stmt, s2} <- parse_statement(s1),
         {:ok, _, s3} <- expect(s2, :rbrace, "}") do
      {:ok, stmt, s3}
    end
  end

  # --- inputs blocks ------------------------------------------------------

  defp parse_optional_inputs(state) do
    case peek(state) do
      %{type: :lbrace} -> parse_inputs_block(state)
      _ -> {:ok, %{}, state}
    end
  end

  defp parse_inputs_block(state) do
    with {:ok, _, s1} <- expect(state, :lbrace, "{"),
         {:ok, pairs, s2} <- parse_input_pairs(s1, %{}),
         {:ok, _, s3} <- expect(s2, :rbrace, "}") do
      {:ok, pairs, s3}
    end
  end

  defp parse_input_pairs(state, acc) do
    case peek(state) do
      %{type: :rbrace} ->
        {:ok, acc, state}

      %{type: :ident, value: key} ->
        with {:ok, _, s1} <- expect(state, :ident, key),
             {:ok, _, s2} <- expect(s1, :colon, ":"),
             {:ok, value, s3} <- parse_pure(s2) do
          {next, s4} = skip_optional_comma(s3)
          _ = next
          parse_input_pairs(s4, Map.put(acc, key, value))
        end

      other ->
        error(state, "expected an input key", token_value(other))
    end
  end

  defp skip_optional_comma(state) do
    case peek(state) do
      %{type: :comma} -> {:comma, advance(state)}
      _ -> {:none, state}
    end
  end

  # --- pure values --------------------------------------------------------

  @keyword_literals %{"true" => true, "false" => false, "null" => nil}

  defp parse_pure(state) do
    case peek(state) do
      %{type: :string} = tok ->
        {:ok, string_to_pure(tok.value), advance(state)}

      %{type: :int, value: n} ->
        {:ok, AST.literal(n), advance(state)}

      %{type: :float, value: f} ->
        {:ok, AST.literal(f), advance(state)}

      %{type: :keyword, value: kw} when is_map_key(@keyword_literals, kw) ->
        {:ok, AST.literal(Map.fetch!(@keyword_literals, kw)), advance(state)}

      %{type: :interp, value: path} ->
        {:ok, interp_to_pure(path), advance(state)}

      %{type: :lbracket} ->
        parse_list(state)

      other ->
        error(state, "expected a value (string, number, boolean, list, or ${ref})", token_value(other))
    end
  end

  defp parse_list(state) do
    with {:ok, _, s1} <- expect(state, :lbracket, "[") do
      parse_list_items(s1, [])
    end
  end

  defp parse_list_items(state, acc) do
    case peek(state) do
      %{type: :rbracket} ->
        {:ok, AST.list(Enum.reverse(acc)), advance(state)}

      _ ->
        with {:ok, item, s1} <- parse_pure(state) do
          {_, s2} = skip_optional_comma(s1)

          case peek(s2) do
            %{type: :rbracket} -> {:ok, AST.list(Enum.reverse([item | acc])), advance(s2)}
            _ -> parse_list_items(s2, [item | acc])
          end
        end
    end
  end

  # A string literal may carry `${path}` interpolations. With none it is a
  # plain literal; with any it lowers to a concat of literal and field
  # segments so the interpreter can resolve the refs at expand time.
  defp string_to_pure(parts) when is_list(parts) do
    case parts do
      [{:lit, text}] ->
        AST.literal(text)

      [] ->
        AST.literal("")

      _ ->
        AST.concat(Enum.map(parts, &segment_to_pure/1))
    end
  end

  defp segment_to_pure({:lit, text}), do: AST.literal(text)
  defp segment_to_pure({:ref, path}), do: interp_to_pure(path)

  # `${name.a.b}` -> field read of binding `name` at path [a, b]; a bare
  # `${name}` is just the binding.
  defp interp_to_pure([name]), do: AST.var(name)
  defp interp_to_pure([name | path]), do: AST.field(AST.var(name), path)

  # --- token helpers ------------------------------------------------------

  defp require_prompt(%{prompt: nil}, state), do: error(state, "agent is missing a prompt field", :missing_prompt)

  defp require_prompt(%{prompt: prompt}, _state), do: {:ok, prompt}

  defp optional_string(state) do
    case peek(state) do
      %{type: :string} = tok -> {:ok, %{tok | value: flatten_string(tok.value)}, advance(state)}
      _ -> {:ok, nil, state}
    end
  end

  defp expect_string(state) do
    case peek(state) do
      %{type: :string} = tok ->
        # A workflow / skill name is a flat literal; interpolation is not
        # meaningful in a name position.
        {:ok, %{tok | value: flatten_string(tok.value)}, advance(state)}

      other ->
        error(state, "expected a string", token_value(other))
    end
  end

  defp flatten_string(parts) do
    Enum.map_join(parts, "", fn
      {:lit, text} -> text
      {:ref, path} -> "${" <> Enum.join(path, ".") <> "}"
    end)
  end

  defp expect_int(state) do
    case peek(state) do
      %{type: :int} = tok -> {:ok, tok, advance(state)}
      other -> error(state, "expected an integer", token_value(other))
    end
  end

  defp expect_ident(state) do
    case peek(state) do
      %{type: :ident} = tok -> {:ok, tok, advance(state)}
      other -> error(state, "expected an identifier", token_value(other))
    end
  end

  defp expect(state, type, literal) do
    case peek(state) do
      %{type: ^type} = tok -> {:ok, tok, advance(state)}
      other -> error(state, "expected #{inspect(literal)}", token_value(other))
    end
  end

  defp skip_to_end(state) do
    case peek(state) do
      :eof -> :ok
      other -> error(state, "unexpected trailing input", token_value(other))
    end
  end

  defp peek(%{tokens: [tok | _]}), do: tok
  defp peek(%{tokens: []}), do: :eof

  defp advance(%{tokens: [_ | rest]} = state), do: %{state | tokens: rest}
  defp advance(%{tokens: []} = state), do: state

  defp token_value(:eof), do: :eof
  defp token_value(%{value: value}), do: value
  defp token_value(other), do: other

  # IR node ids must be stable across a deterministic replay. The parser
  # assigns each effect a monotonically increasing positional id; the
  # interpreter combines it with an expansion key to derive the final
  # IR.Node id, so re-parsing the same source yields the same ids.
  defp next_id(state, kind) do
    n = state.counter
    {"#{kind}-#{n}", %{state | counter: n + 1}}
  end

  defp error(state, message, got) do
    {line, column} = span_at(state)
    {:error, %{message: message, line: line, column: column, file: Map.get(state, :file), got: got}}
  end

  defp span_at(%{tokens: [%{line: line, column: column} | _]}), do: {line, column}
  defp span_at(%{tokens: []}), do: {1, 1}
end
