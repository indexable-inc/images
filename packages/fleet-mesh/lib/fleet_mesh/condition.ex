defmodule FleetMesh.Condition do
  @moduledoc """
  One thing worth knowing about the fleet, as data.

  A condition names a check, says how often to run it and how loudly to
  report it, and nothing else. It carries no threshold, no host name and no
  query: those live in whatever module implements `FleetMesh.Policy` and
  builds these structs. Keeping conditions inert data is what lets the same
  engine drive a public empty policy, a private catalog, and a test policy
  with a flippable check, without any of them knowing about each other.

  Composition is by ordinary function. `from_query/2` builds one from a
  reader; a policy that wants a family of conditions writes a comprehension
  over `new/1`. There is deliberately no macro DSL -- a list of structs is
  already inspectable, testable and diffable, and a macro would only make it
  less so.

  ## Three states, because a check that could not run is not a check that
  ## passed

  `t:state/0` is `:green | :red | :unknown`, and that set is taken from the
  code this engine replaces rather than invented here. `IxMcp.Fleet.Alerts`
  types every predicate as `{:ok, [hit()]} | {:error, String.t()}`, which is
  exactly these three: no hits, hits, and could-not-read. Its poller keeps
  `errors` as a separate key from `announced` for the stated reason that a
  fleet nobody could read is not a healthy fleet.

  There is no `:amber`. Nothing in the alert catalog, the poller or the
  digest has an intermediate *state*; what looks like one is the RFC 5424
  severity carried on an already-fired condition, which is `severity` here
  and is orthogonal to whether the condition is currently true.
  """

  @typedoc """
  What the last evaluation concluded.

  * `:green` -- the check ran and the condition is not true.
  * `:red` -- the check ran and the condition is true.
  * `:unknown` -- the check did not produce an answer. A read that failed, a
    check that raised, and a check that ran past its deadline all land here,
    each with the reason in `detail`.
  """
  @type state :: :green | :red | :unknown

  @typedoc "RFC 5424, the same vocabulary the MCP `logging/setLevel` floor uses."
  @type severity ::
          :debug | :info | :notice | :warning | :error | :critical | :alert | :emergency

  @typedoc """
  Work that answers the question. Either a zero-arity fun or an MFA, so a
  policy can be a plain module with no closures in its config.
  """
  @type check :: (-> result()) | {module(), atom(), [term()]}

  @typedoc """
  What a check returns. `detail` is opaque to the engine and is passed
  through to subscribers untouched: rows, a message, a struct, anything.

  A bare `:green`/`:red`/`:unknown` is accepted as shorthand for that state
  with a `nil` detail.
  """
  @type result :: {state(), term()} | state()

  @type t :: %__MODULE__{
          id: atom(),
          severity: severity(),
          description: String.t(),
          interval_ms: pos_integer(),
          check: check()
        }

  @enforce_keys [:id, :severity, :description, :interval_ms, :check]
  defstruct [:id, :severity, :description, :interval_ms, :check]

  @severities [:debug, :info, :notice, :warning, :error, :critical, :alert, :emergency]

  @doc """
  Build a condition, refusing anything the engine could not run.

  Every field is checked here rather than at evaluation time, because a
  policy with one malformed condition should fail when it is loaded -- while
  a human is looking at it -- and not four hours later as one silently
  missing signal among twenty working ones.
  """
  @spec new(keyword() | map()) :: t()
  def new(attrs) do
    attrs = Map.new(attrs)

    %__MODULE__{
      id: validate!(:id, Map.get(attrs, :id), &is_atom/1, "an atom"),
      severity:
        validate!(:severity, Map.get(attrs, :severity), &(&1 in @severities), severities()),
      description:
        validate!(:description, Map.get(attrs, :description), &is_binary/1, "a string"),
      interval_ms:
        validate!(
          :interval_ms,
          Map.get(attrs, :interval_ms),
          &(is_integer(&1) and &1 > 0),
          "a positive integer of milliseconds"
        ),
      check:
        validate!(
          :check,
          Map.get(attrs, :check),
          &valid_check?/1,
          "a zero-arity fun or an {module, function, args} tuple"
        )
    }
  end

  @doc """
  Build a condition from a reader returning the `{:ok, rows} | {:error, reason}`
  shape that every fleet query already speaks.

  The mapping is the whole point and is not negotiable per policy: no rows is
  `:green`, any rows are `:red` with the rows as detail, and a failed read is
  `:unknown` with the reason. A policy that mapped a failed read to `:green`
  would be reporting health it never measured.

  `attrs` takes the same keys as `new/1` minus `:check`.
  """
  @spec from_query(keyword() | map(), (-> {:ok, list()} | {:error, term()})) :: t()
  def from_query(attrs, reader) when is_function(reader, 0) do
    attrs
    |> Map.new()
    |> Map.put(:check, fn ->
      case reader.() do
        {:ok, []} -> {:green, []}
        {:ok, rows} when is_list(rows) -> {:red, rows}
        {:error, reason} -> {:unknown, reason}
        other -> {:unknown, {:unexpected_reader_return, other}}
      end
    end)
    |> new()
  end

  @doc """
  Run a condition's check and normalise whatever comes back into a
  `{state, detail}` pair.

  A check that raises, throws or exits is caught here and becomes
  `:unknown`, carrying the kind and reason. That is the difference between a
  broken check and a passing one, and letting it escape would instead take
  down the engine process and stop every other condition with it.
  """
  @spec evaluate(t()) :: {state(), term()}
  def evaluate(%__MODULE__{check: check}) do
    normalise(run(check))
  catch
    kind, reason -> {:unknown, {kind, reason, __STACKTRACE__}}
  end

  @doc "The severities `new/1` accepts, in RFC 5424 order."
  @spec severities() :: [severity()]
  def severities, do: @severities

  @spec run(check()) :: result()
  defp run(check) when is_function(check, 0), do: check.()
  defp run({module, function, args}), do: apply(module, function, args)

  @spec normalise(result()) :: {state(), term()}
  defp normalise({state, detail}) when state in [:green, :red, :unknown], do: {state, detail}
  defp normalise(state) when state in [:green, :red, :unknown], do: {state, nil}
  defp normalise(other), do: {:unknown, {:unexpected_check_return, other}}

  @spec valid_check?(term()) :: boolean()
  defp valid_check?(check) when is_function(check, 0), do: true

  defp valid_check?({module, function, args})
       when is_atom(module) and is_atom(function) and is_list(args),
       do: true

  defp valid_check?(_other), do: false

  @spec validate!(atom(), term(), (term() -> boolean()), term()) :: term()
  defp validate!(key, value, valid?, expected) do
    if valid?.(value) do
      value
    else
      raise ArgumentError,
            "FleetMesh.Condition #{inspect(key)} must be #{inspect(expected)}, got: " <>
              inspect(value)
    end
  end
end
