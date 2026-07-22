defmodule IxMcp.Memory do
  @moduledoc """
  Durable agent memory over a weave store (append-only fact journal with
  Datalog-derived views), aliased in the workspace prelude as `Memory`.
  Every call is a one-shot `weave` CLI invocation against the store named
  by the WEAVE_MEMORY_STORE environment variable; no daemon owns the
  store. The one resident process is `semantic/2`'s embedding recall
  (`IxMcp.Memory.Semantic` keeps `weave recall --stdin` warm because
  model load dominates its latency; it opens the store read-only). A
  host without the env set gets a loud error naming the knob and pays
  nothing otherwise.

  Conventions (the SessionStart digest is derived from these): entities
  are `mem:<slug>`; `mem/desc` is the one-line hook, `mem/type` one of
  user | feedback | project | reference, `mem/topic` a repeatable tag,
  `mem/handle` the concrete command/path/flag, `mem/body` a blake3 CAS
  hash of long-form content, `mem/verified-at` a re-check receipt
  (UTC timestamp plus provenance, see `verify/2`). `mem/supersedes` and
  `mem/relates` hold other `mem:` entities: weave treats entity-valued
  facts as typed edges, so corrections and clusters are walkable via
  `graph/1` instead of discoverable only by regex. History is never
  edited: newer facts win, `retract/1` kills a wrong one.
  """

  alias IxMcp.Memory.Semantic

  @recall_limit 20
  @semantic_limit 8

  @doc """
  Save a memory: `Memory.remember("slug", "hook", type: "project",
  topic: ["nix", "fleet"], handle: "nix run .#lint", body: long_markdown,
  supersedes: "old-slug", relates: ["peer-slug"])`.

  `supersedes:` and `relates:` take slugs or `mem:` names and are written
  as entity-valued facts, which weave treats as typed edges (`graph/1`
  walks them). `supersedes:` makes a correction explicit instead of
  relying on newer-wins; `relates:` links peers.
  """
  @spec remember(String.t(), String.t(), keyword()) :: :ok
  def remember(slug, desc, opts \\ []) do
    entity = mem_entity(slug)
    fact(entity, "mem/desc", desc)

    for key <- [:type, :topic, :handle],
        value <- List.wrap(Keyword.get(opts, key)) do
      fact(entity, "mem/#{key}", to_string(value))
    end

    for key <- [:supersedes, :relates],
        value <- List.wrap(Keyword.get(opts, key)) do
      fact(entity, "mem/#{key}", mem_entity(to_string(value)))
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

  @doc """
  Case-insensitive regex search over slugs, hooks, and topics: whole-word
  by default (`match: :substring` opts out, so `recall("hil")` stops
  matching every hook containing "while"), newest first, `limit: 20`.

  Each row carries what trusting, fetching, or killing the memory needs:
  `%{entity, id, seq, time, desc, type, topic, handle, body, verified_at}`.
  `id` feeds `retract/1`, `seq`/`time` show staleness, `topic` is the
  list of live tags, `body` is the CAS content resolved via `weave get`,
  and `verified_at` is the latest re-check receipt (nil when never
  verified, see `verify/2`).
  """
  @spec recall(String.t(), keyword()) :: [map()]
  def recall(pattern, opts \\ []) do
    opts = Keyword.validate!(opts, limit: @recall_limit, match: :word)
    limit = Keyword.fetch!(opts, :limit)

    rx =
      case Keyword.fetch!(opts, :match) do
        :word -> dl_string("(?i)\\b(?:#{pattern})\\b")
        :substring -> dl_string("(?i)" <> pattern)
      end

    hits =
      query("""
      hit(E) :- latest(E, "mem/desc", D), regex(D, "#{rx}").
      hit(E) :- fact(E, "mem/topic", T), regex(T, "#{rx}").
      hit(E) :- latest(E, "mem/desc", _), regex(E, "#{rx}").
      row(S, T, I, E, D) :- hit(E), latest(E, "mem/desc", D), fact_id(I, E, "mem/desc", D), fact_seq(I, S), fact_time(I, T).
      ?- row(S, T, I, E, D) order S desc limit #{limit}.
      """)

    attrs = attrs(Enum.map(hits, & &1["E"]))

    for %{"E" => entity} = row <- Enum.uniq_by(hits, & &1["E"]) do
      enriched_row(entity, row, Map.get(attrs, entity, %{}))
    end
  end

  @doc """
  Embedding-similarity recall: ranks entities by exact cosine between the
  query and their composed documents (qwen3-embedding-4b by default,
  WEAVE_EMBED_MODEL overrides), so it finds what `recall/2` misses when
  no word matches. `limit: 8` entry points by default.

  Rows are `recall/2`'s enriched shape plus `similarity` (cosine, higher
  is closer), best first. A hit without `mem/desc` (the store embeds
  non-`mem:` entities too) keeps `entity` and `similarity`, borrows the
  embedded document's label as `desc`, and carries the fact-derived
  fields as nil. Superseded memories never surface: weave drops
  `mem/supersedes` targets from the search matrix.

  The first call pays the model load (~1.2s warm-cache); later calls
  ride the resident `weave recall --stdin` process (~76ms, see
  `IxMcp.Memory.Semantic`). Needs a weave with semantic recall
  (indexable-inc/weave#339).
  """
  @spec semantic(String.t(), keyword()) :: [map()]
  def semantic(query, opts \\ []) do
    opts = Keyword.validate!(opts, limit: @semantic_limit)
    entries = Semantic.entries(query, Keyword.fetch!(opts, :limit))
    entities = Enum.map(entries, & &1["entity"])
    descs = desc_rows(entities)
    attrs = attrs(entities)

    for %{"entity" => entity, "similarity" => similarity} = entry <- entries do
      row =
        case descs do
          %{^entity => desc_row} -> enriched_row(entity, desc_row, Map.get(attrs, entity, %{}))
          _ -> label_row(entity, entry["label"])
        end

      Map.put(row, :similarity, similarity)
    end
  end

  @doc """
  The typed-edge neighborhood of `mem:<slug>`: every live entity-valued
  fact (mem/supersedes, mem/relates, any future edge attribute) pointing
  into or out of the entity, as `%{from, edge, to}` rows.
  """
  @spec graph(String.t()) :: [map()]
  def graph(slug) do
    anchor = dl_string("^" <> Regex.escape(mem_entity(slug)) <> "$")

    rows =
      query("""
      edge(S, A, O) :- fact(S, A, O), regex(O, "^mem:").
      hit(S, A, O) :- edge(S, A, O), regex(S, "#{anchor}").
      hit(S, A, O) :- edge(S, A, O), regex(O, "#{anchor}").
      ?- hit(S, A, O).
      """)

    for %{"S" => from, "A" => edge, "O" => to} <- rows, do: %{from: from, edge: edge, to: to}
  end

  @doc """
  Record that a memory was re-checked against reality: appends a
  `mem/verified-at` fact whose value is the UTC timestamp plus a
  provenance string (`session:` overrides; the default is the kernel
  session name, else CLAUDE_SESSION_ID, else user@host). `recall/2` rows
  surface the receipt as `verified_at` and the SessionStart digest ranks
  verified memories first, so verifying keeps a memory near the top.
  Returns the fact id.
  """
  @spec verify(String.t(), keyword()) :: String.t()
  def verify(slug, opts \\ []) do
    session = Keyword.fetch!(Keyword.validate!(opts, session: nil), :session) || provenance()
    stamp = DateTime.utc_now() |> DateTime.truncate(:second) |> DateTime.to_iso8601()
    fact(mem_entity(slug), "mem/verified-at", "#{stamp} #{session}")
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

  @doc "Fetch a CAS blob by `blake3:<hex>` hash (verifies integrity)."
  @spec get(String.t()) :: String.t()
  def get(hash), do: run!(["get", hash])

  # One query for every live `mem/` fact on the hit entities, keyed
  # entity -> attribute -> [{seq, value}]. Sourced from fact/3 rather than
  # latest/3 because latest keeps one value per (entity, attribute), which
  # would collapse repeatable topics to the newest tag; latest-per-attribute
  # for the scalar attributes is resolved in `newest_value/2`.
  @spec attrs([String.t()]) :: %{String.t() => %{String.t() => [{integer(), String.t()}]}}
  defp attrs([]), do: %{}

  defp attrs(entities) do
    rows =
      query("""
      attr(S, E, A, V) :- fact(E, A, V), fact_id(I, E, A, V), fact_seq(I, S), regex(E, "#{anchor(entities)}"), regex(A, "^mem/").
      ?- attr(S, E, A, V).
      """)

    rows
    |> Enum.group_by(& &1["E"])
    |> Map.new(fn {entity, entity_rows} ->
      {entity, Enum.group_by(entity_rows, & &1["A"], &{&1["S"], &1["V"]})}
    end)
  end

  # The `mem/desc` anchor rows (id, seq, time, hook) for exactly these
  # entities, keyed by entity: semantic hits arrive as bare ids, unlike
  # recall's regex hits, which carry their row already.
  @spec desc_rows([String.t()]) :: %{String.t() => map()}
  defp desc_rows([]), do: %{}

  defp desc_rows(entities) do
    rows =
      query("""
      row(S, T, I, E, D) :- latest(E, "mem/desc", D), fact_id(I, E, "mem/desc", D), fact_seq(I, S), fact_time(I, T), regex(E, "#{anchor(entities)}").
      ?- row(S, T, I, E, D).
      """)

    Map.new(rows, &{&1["E"], &1})
  end

  # The rich row recall/2 and semantic/2 share: identity fields from the
  # mem/desc anchor fact, the rest from the entity's live mem/ attributes.
  @spec enriched_row(String.t(), map(), %{String.t() => [{integer(), String.t()}]}) :: map()
  defp enriched_row(entity, %{"D" => desc, "I" => id, "S" => seq, "T" => time}, entity_attrs) do
    %{
      entity: entity,
      id: id,
      seq: seq,
      time: DateTime.from_unix!(time, :millisecond),
      desc: desc,
      type: newest_value(entity_attrs, "mem/type"),
      topic: live_values(entity_attrs, "mem/topic"),
      handle: newest_value(entity_attrs, "mem/handle"),
      body: body_text(entity_attrs),
      verified_at: newest_value(entity_attrs, "mem/verified-at")
    }
  end

  # Same shape for a semantic hit that has no mem/desc fact, so callers
  # can treat every row uniformly.
  @spec label_row(String.t(), String.t() | nil) :: map()
  defp label_row(entity, label) do
    %{
      entity: entity,
      id: nil,
      seq: nil,
      time: nil,
      desc: label,
      type: nil,
      topic: [],
      handle: nil,
      body: nil,
      verified_at: nil
    }
  end

  # Regex matching exactly these entity ids, escaped for a Datalog string.
  @spec anchor([String.t()]) :: String.t()
  defp anchor(entities) do
    entities
    |> Enum.uniq()
    |> Enum.map_join("|", &Regex.escape/1)
    |> then(&dl_string("^(?:#{&1})$"))
  end

  @spec body_text(%{String.t() => [{integer(), String.t()}]}) :: String.t() | nil
  defp body_text(by_attr) do
    case newest_value(by_attr, "mem/body") do
      nil -> nil
      hash -> get(hash)
    end
  end

  @spec newest_value(%{String.t() => [{integer(), String.t()}]}, String.t()) ::
          String.t() | nil
  defp newest_value(by_attr, attr) do
    case Map.get(by_attr, attr) do
      nil -> nil
      pairs -> pairs |> Enum.max_by(fn {seq, _value} -> seq end) |> elem(1)
    end
  end

  @spec live_values(%{String.t() => [{integer(), String.t()}]}, String.t()) :: [String.t()]
  defp live_values(by_attr, attr) do
    by_attr
    |> Map.get(attr, [])
    |> Enum.sort()
    |> Enum.map(fn {_seq, value} -> value end)
    |> Enum.uniq()
  end

  @spec mem_entity(String.t()) :: String.t()
  defp mem_entity("mem:" <> _rest = entity), do: entity
  defp mem_entity(slug), do: "mem:" <> slug

  # Escape for embedding in a weave Datalog double-quoted string literal.
  @spec dl_string(String.t()) :: String.t()
  defp dl_string(s), do: s |> String.replace("\\", "\\\\") |> String.replace("\"", "\\\"")

  @spec provenance() :: String.t()
  defp provenance do
    kernel_session() || System.get_env("CLAUDE_SESSION_ID") || user_at_host()
  end

  # The kernel names sessions via IxMcp.Session; outside a running kernel
  # (plain IEx) the process is absent or the session unnamed, so fall through.
  @spec kernel_session() :: String.t() | nil
  defp kernel_session do
    case Process.whereis(IxMcp.Session) do
      nil -> nil
      _pid -> IxMcp.Session.get().name
    end
  end

  @spec user_at_host() :: String.t()
  defp user_at_host do
    {:ok, host} = :inet.gethostname()
    "#{System.get_env("USER") || "unknown"}@#{host}"
  end

  @doc false
  @spec store!() :: String.t()
  def store! do
    System.get_env("WEAVE_MEMORY_STORE") ||
      raise "WEAVE_MEMORY_STORE is unset: point it at a weave store " <>
              "(create one with `weave --store <dir> init`) to enable Memory"
  end

  @doc false
  @spec weave_bin!() :: String.t()
  def weave_bin! do
    System.get_env("WEAVE_BIN") || System.find_executable("weave") ||
      raise "weave binary not found: install weave on PATH or set WEAVE_BIN"
  end

  @spec run!([String.t()]) :: String.t()
  defp run!(args) do
    # Store before binary: WEAVE_MEMORY_STORE is the setup knob the module
    # doc leads with, so when both are missing the error must name it. The
    # sandboxed check stages no weave binary, so the reverse order failed
    # there while passing on any dev machine with weave installed.
    store = store!()

    case System.cmd(weave_bin!(), ["--store", store] ++ args, stderr_to_stdout: true) do
      {out, 0} -> out
      {out, code} -> raise "weave #{Enum.join(args, " ")} exited #{code}: #{out}"
    end
  end
end
