defmodule IxMcp.Memory do
  @moduledoc """
  Durable agent memory over a weave store (append-only fact journal with
  Datalog-derived views), aliased in the workspace prelude as `Memory`.
  Every call is a one-shot `weave` CLI invocation against the store named
  by the WEAVE_MEMORY_STORE environment variable; there is no daemon. A
  host without the env set gets a loud error naming the knob and pays
  nothing otherwise.

  Conventions (the SessionStart digest is derived from these): entities
  are `mem:<slug>`; `mem/desc` is the one-line hook, `mem/type` one of
  user | feedback | project | reference, `mem/topic` a repeatable tag,
  `mem/handle` the concrete command/path/flag, `mem/body` a blake3 CAS
  hash of long-form content. History is never edited: newer facts win,
  `retract/1` kills a wrong one.
  """

  @doc """
  Save a memory: `Memory.remember("slug", "hook", type: "project",
  topic: ["nix", "fleet"], handle: "nix run .#lint", body: long_markdown)`.
  """
  @spec remember(String.t(), String.t(), keyword()) :: :ok
  def remember(slug, desc, opts \\ []) do
    entity = "mem:" <> slug
    fact(entity, "mem/desc", desc)

    for key <- [:type, :topic, :handle],
        value <- List.wrap(Keyword.get(opts, key)) do
      fact(entity, "mem/#{key}", to_string(value))
    end

    case Keyword.get(opts, :body) do
      nil -> :ok
      body -> fact(entity, "mem/body", put(body))
    end

    :ok
  end

  @doc "Append one fact; returns the fact id (`blake3:<hex>`)."
  @spec fact(String.t(), String.t(), String.t()) :: String.t()
  def fact(entity, attr, value) do
    # CLI writes need the exclusive offline writer lease (no daemon owns the
    # store here); `--` keeps values that start with `-` positional.
    # `weave fact` prints "seq <n>  <fact-id>".
    ["fact", "--offline", "--", entity, attr, value] |> run!() |> String.split() |> List.last()
  end

  @doc "Run a Datalog query program; returns the decoded result rows."
  @spec query(String.t()) :: [map()]
  def query(program) do
    {:ok, %{"rows" => rows}} = ["--json", "query", program] |> run!() |> JSON.decode()
    rows
  end

  @doc "Case-insensitive regex search over slugs, hooks, and topics."
  @spec recall(String.t()) :: [map()]
  def recall(pattern) do
    p = pattern |> String.replace("\\", "\\\\") |> String.replace("\"", "\\\"")

    query("""
    hit(E, D) :- latest(E, "mem/desc", D), regex(D, "(?i)#{p}").
    hit(E, D) :- latest(E, "mem/topic", T), regex(T, "(?i)#{p}"), latest(E, "mem/desc", D).
    hit(E, D) :- regex(E, "(?i)#{p}"), latest(E, "mem/desc", D).
    ?- hit(E, D).
    """)
  end

  @doc "Retract a wrong fact by id (staleness prefers newer facts instead)."
  @spec retract(String.t()) :: String.t()
  def retract(fact_id), do: ["retract", "--offline", "--", fact_id] |> run!() |> String.trim()

  @doc "Store long-form content in the CAS; returns its `blake3:<hex>`."
  @spec put(String.t()) :: String.t()
  def put(content) do
    path = Path.join(System.tmp_dir!(), "weave-put-#{System.unique_integer([:positive])}")
    File.write!(path, content)

    try do
      ["put", path] |> run!() |> String.trim()
    after
      File.rm(path)
    end
  end

  @spec run!([String.t()]) :: String.t()
  defp run!(args) do
    store =
      System.get_env("WEAVE_MEMORY_STORE") ||
        raise "WEAVE_MEMORY_STORE is unset: point it at a weave store " <>
                "(create one with `weave --store <dir> init`) to enable Memory"

    bin =
      System.get_env("WEAVE_BIN") || System.find_executable("weave") ||
        raise "weave binary not found: install weave on PATH or set WEAVE_BIN"

    case System.cmd(bin, ["--store", store] ++ args, stderr_to_stdout: true) do
      {out, 0} -> out
      {out, code} -> raise "weave #{Enum.join(args, " ")} exited #{code}: #{out}"
    end
  end
end
