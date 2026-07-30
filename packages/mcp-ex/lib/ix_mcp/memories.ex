defmodule IxMcp.Memories do
  @moduledoc """
  Repo-local agent memory: one markdown file per memory under a repo's
  `.memories/`, read and written through the `memories` CLI
  (`packages/memories`, index#4433), aliased in the workspace prelude as
  `Memories`. Every call is a one-shot `memories` invocation; no daemon
  owns the directory, and the CLI is the format's only writer, so nothing
  here renders frontmatter. A host without the binary gets a loud error
  naming the `MEMORIES_BIN` knob and pays nothing otherwise.

  `search/2` returns ranked hits and there is deliberately no `rank/1`:
  two names for one result is how a caller ends up sorting twice. A caller
  that wants the unranked lexical order reads `bm25` off each hit and
  sorts it itself. Nothing here reorders what the CLI ranked.

  Hits are `%IxMcp.Memories.Hit{}` structs, not maps: the fields are typed
  and a key the CLI stopped emitting raises here instead of surfacing as a
  `nil` three calls later. `expand/2` takes and returns hits.

  A search answers with the directories it read, not only the hits it
  found: `search/2` returns a `%IxMcp.Memories.Results{}` carrying `roots`
  and `scanned` next to `hits`, because zero hits from a root set that
  silently resolved to one unexpected directory is indistinguishable from
  zero hits from the right directories. Each root is a `%Root{}` row with
  its own `memories` count, so `memories: 0` everywhere names a coverage
  problem and a healthy count names a genuine miss. `roots/1` answers the
  same question without a query.

  Zero hits is normal, because the CLI has a score floor and returns
  nothing rather than its best of a bad set. Nothing here treats an empty
  result as failure.

  Discovery is relative to the working directory (the `.memories` of its
  git toplevel, then each parent toplevel, then `~/.memories`), and the OS
  cwd is BEAM-global -- any cell can move it -- so every call runs from
  `IxMcp.Cmd`'s immutable launch directory unless `cd:` says otherwise.
  `dirs:` is a list of `.memories` directories and replaces that default
  resolution entirely rather than adding to it; it is a list even when it
  names one directory, so the plural path is the tested one. Defaults for
  `--limit` and `--days` live in the CLI alone: an option this module was
  not given is not passed.

  Genres are `memory | living | recipe | historical | frozen`, decoded to
  atoms. Corrections are supersessions, never edits: `remember/3` with
  `supersedes:` writes the new memory and leaves the old one readable,
  which is what makes a wrong memory auditable rather than gone.
  """

  alias IxMcp.Cmd

  defmodule Validation do
    @moduledoc """
    One `validated:` receipt on a memory: when it was re-checked, by whom,
    the command that proves it, and whether that command agreed. `how` is
    the load-bearing field, because it is what a later reader re-runs.
    """

    @type t :: %__MODULE__{
            at: DateTime.t(),
            by: String.t(),
            how: String.t(),
            ok: boolean()
          }

    defstruct [:at, :by, :how, :ok]

    @doc false
    @spec from_json!(map(), String.t()) :: t()
    def from_json!(%{"at" => at, "by" => by, "how" => how, "ok" => ok}, slug) do
      %__MODULE__{at: stamp!(at, slug), by: by, how: how, ok: ok}
    end

    # The CLI is the only writer of this field, so an unparseable timestamp
    # is a bug in the writer or a hand-edit, not an input to tolerate.
    @spec stamp!(String.t(), String.t()) :: DateTime.t()
    defp stamp!(at, slug) do
      case DateTime.from_iso8601(at) do
        {:ok, stamp, _offset} ->
          stamp

        {:error, reason} ->
          raise "memory #{slug}: validated.at #{inspect(at)} is not an ISO 8601 " <>
                  "timestamp (#{reason})"
      end
    end
  end

  defmodule Hit do
    @moduledoc """
    One decoded memory. `bm25` and `score` are the ranking fields
    `search/2` returns and `show/1` does not emit, so they are nil for a
    `show`. `stale` means a `based_on` file no longer hashes to the
    recorded value (`stale_reason` names it) -- a stale memory is still
    returned, because reading one unflagged is the harm.
    """

    @type genre :: :memory | :living | :recipe | :historical | :frozen

    @type t :: %__MODULE__{
            slug: String.t(),
            path: String.t(),
            root: String.t(),
            tldr: String.t(),
            genre: genre(),
            topic: [String.t()],
            handle: [String.t()],
            prior: number(),
            related: [String.t()],
            supersedes: [String.t()],
            scope: String.t(),
            bm25: number() | nil,
            score: number() | nil,
            stale: boolean(),
            stale_reason: String.t() | nil,
            refuted: boolean(),
            validated: [Validation.t()],
            body: String.t()
          }

    defstruct [
      :slug,
      :path,
      :root,
      :tldr,
      :genre,
      :topic,
      :handle,
      :prior,
      :related,
      :supersedes,
      :scope,
      :bm25,
      :score,
      :stale,
      :stale_reason,
      :refuted,
      :validated,
      :body
    ]

    @doc false
    @spec from_json!(map()) :: t()
    def from_json!(%{"slug" => slug} = hit) do
      %__MODULE__{
        slug: slug,
        path: Map.fetch!(hit, "path"),
        root: Map.fetch!(hit, "root"),
        tldr: Map.fetch!(hit, "tldr"),
        genre: genre!(Map.fetch!(hit, "genre"), slug),
        topic: Map.fetch!(hit, "topic"),
        handle: Map.fetch!(hit, "handle"),
        prior: Map.fetch!(hit, "prior"),
        related: Map.fetch!(hit, "related"),
        supersedes: Map.fetch!(hit, "supersedes"),
        scope: Map.fetch!(hit, "scope"),
        # Absent on a `show`, which emits no ranking fields.
        bm25: Map.get(hit, "bm25"),
        score: Map.get(hit, "score"),
        stale: Map.fetch!(hit, "stale"),
        stale_reason: Map.get(hit, "stale_reason"),
        refuted: Map.fetch!(hit, "refuted"),
        validated: Enum.map(Map.fetch!(hit, "validated"), &Validation.from_json!(&1, slug)),
        body: Map.fetch!(hit, "body")
      }
    end

    # Spelled out rather than String.to_existing_atom/1: the set is closed
    # by the file format, and an unknown genre names the file it came from.
    @spec genre!(String.t(), String.t()) :: genre()
    defp genre!("memory", _slug), do: :memory
    defp genre!("living", _slug), do: :living
    defp genre!("recipe", _slug), do: :recipe
    defp genre!("historical", _slug), do: :historical
    defp genre!("frozen", _slug), do: :frozen

    defp genre!(other, slug) do
      raise "memory #{slug}: unknown genre #{inspect(other)}; expected one of " <>
              "memory, living, recipe, historical, frozen"
    end
  end

  defmodule Root do
    @moduledoc """
    One resolved search root: where it is, whether it is there, and how many
    memories it holds.

    Not a bare path, and the count is the reason. A caller looking at zero
    hits has to tell "the roots I expected, and they are empty" apart from
    "a root set that resolved somewhere unexpected", and a path alone cannot
    say which: `memories: 0` on every row is unmistakably a coverage
    problem, while a healthy count next to zero hits is a genuine miss.
    `exists: false` is normal for a default root -- a repo with no
    `.memories` yet is skipped silently -- and is a typo when the caller
    named that root itself, which the CLI refuses outright.
    """

    @type t :: %__MODULE__{
            path: String.t(),
            exists: boolean(),
            memories: non_neg_integer()
          }

    defstruct [:path, :exists, :memories]

    @doc false
    @spec from_json!(map()) :: t()
    def from_json!(%{"path" => path, "exists" => exists, "memories" => memories}) do
      %__MODULE__{path: path, exists: exists, memories: memories}
    end
  end

  defmodule Results do
    @moduledoc """
    What a search answered and where it looked: the ranked `hits`, the
    `roots` it resolved as `%Root{}` rows, the number of memories `scanned`
    and `elapsed_ms`.

    The roots ride with the result rather than waiting for a caller to
    think of asking, because zero hits from a root set that silently
    resolved to one unexpected directory is indistinguishable from zero
    hits from the right directories, and the hits alone cannot tell the
    two apart. Each row carries its own `memories` count, which is what
    separates those two cases; see `Root`.

    Zero hits is an answer, not an error: `search` drops anything below its
    score floor rather than returning the least-bad match, so empty `hits`
    against healthy `roots` means the corpus has nothing good, and a caller
    should say so rather than act on a weak hit.
    """

    @type t :: %__MODULE__{
            query: String.t(),
            roots: [Root.t()],
            scanned: non_neg_integer(),
            elapsed_ms: non_neg_integer(),
            hits: [Hit.t()]
          }

    defstruct [:query, :roots, :scanned, :elapsed_ms, :hits]

    @doc false
    @spec from_json!(map()) :: t()
    def from_json!(payload) do
      %__MODULE__{
        query: Map.fetch!(payload, "query"),
        roots: Enum.map(Map.fetch!(payload, "roots"), &Root.from_json!/1),
        scanned: Map.fetch!(payload, "scanned"),
        elapsed_ms: Map.fetch!(payload, "elapsed_ms"),
        hits: Enum.map(Map.fetch!(payload, "hits"), &Hit.from_json!/1)
      }
    end
  end

  defmodule Row do
    @moduledoc """
    A row from the review commands (`stale/1`, `refuted/1`,
    `unchecked/1`): the memory and one sentence saying what is wrong with
    it. Deliberately not a `Hit`: these commands answer "what needs
    attention", so they carry no ranking and no body.
    """

    @type t :: %__MODULE__{
            slug: String.t(),
            path: String.t(),
            tldr: String.t(),
            reason: String.t()
          }

    defstruct [:slug, :path, :tldr, :reason]

    @doc false
    @spec from_json!(map()) :: t()
    def from_json!(%{"slug" => slug, "path" => path, "tldr" => tldr, "reason" => reason}) do
      %__MODULE__{slug: slug, path: path, tldr: tldr, reason: reason}
    end
  end

  defmodule Diagnostic do
    @moduledoc """
    One `lint/1` finding: the file, the frontmatter line when the rule
    knows it, the rule id (`memory-tldr`, `memory-topic-unknown`, ...) and
    the message. Every rule is an error; there are no lint warnings.
    `memory-body-budget` and `memory-unchecked` fire on `genre: memory`
    only, so a long reference page and its 180-day clock are not findings.
    """

    @type t :: %__MODULE__{
            path: String.t(),
            line: pos_integer() | nil,
            rule: String.t(),
            message: String.t()
          }

    defstruct [:path, :line, :rule, :message]

    @doc false
    @spec from_json!(map()) :: t()
    def from_json!(%{"path" => path, "rule" => rule, "message" => message} = diagnostic) do
      %__MODULE__{path: path, line: Map.get(diagnostic, "line"), rule: rule, message: message}
    end
  end

  @typedoc "A `lint/1` report: every diagnostic, plus what was checked."
  @type lint_report :: %{
          diagnostics: [Diagnostic.t()],
          errors: non_neg_integer(),
          checked: non_neg_integer()
        }

  @doc """
  Ranked search: `Memories.search("why did every host rebuild").hits`, or
  `Memories.search("rebuild", topic: :nix, dirs: ["/a/.memories", "/b/.memories"])`.

  Returns a `%Memories.Results{}` rather than a bare list, so the
  directories the search actually read ride along with the hits it found:
  zero hits and a `roots` you did not expect are the same value until you
  can see both. `expand/2` takes the `hits` off it.

  Hits come back in the CLI's ranked order (relevance times prior times
  genre times recency times validation count, ties broken by slug
  ascending); this function does not re-sort them, and there is no `rank/1`
  to sort them a second way. Re-sorting would also throw away the tie-break
  that makes `limit:` reproducible across runs.

  An empty result is a legitimate answer, not an error: the CLI drops
  anything below its score floor rather than handing back the least-bad
  match. Read `roots` to tell "nothing good in the corpus" from "searched
  the wrong directories". A refuted memory (newest `validated` entry
  `ok: false`) is excluded unless `all: true`.

  `dirs:` is a list of `.memories` directories and replaces the default
  resolution rather than adding to it, so a search naming its roots reads
  exactly those and inherits nothing. `topic:` and `genre:` take one value
  or a list.
  """
  @spec search(String.t(), keyword()) :: Results.t()
  def search(query, opts \\ []) do
    opts =
      Keyword.validate!(opts, limit: nil, topic: [], genre: [], all: false, dirs: [], cd: nil)

    flags =
      option("--limit", opts[:limit]) ++
        options("--topic", opts[:topic]) ++
        options("--genre", opts[:genre]) ++
        switch("--all", opts[:all])

    Results.from_json!(json!("search", flags, [query], opts))
  end

  @doc """
  Follow `related:` edges out of `hits` (`Memories.search(q).hits`),
  `depth: 1` by default: the neighbours are appended in discovery order,
  so the ranked hits stay at the front and nothing is fetched twice.

  Each neighbour is read from the `.memories` root its referrer came from
  (a memory may sit one level deep in a grouping subdirectory, which is not
  a root). A slug is a file stem, never a path, so a neighbour resolves
  wherever in that root it lives. A `related:` slug that does not resolve
  raises; `lint/1` reports exactly that as `memory-related-unresolved`.
  """
  @spec expand([Hit.t()], keyword()) :: [Hit.t()]
  def expand(hits, opts \\ []) do
    opts = Keyword.validate!(opts, depth: 1, cd: nil)
    grow(hits, Keyword.fetch!(opts, :depth), opts[:cd])
  end

  @doc """
  The roots a call with no `dirs:` would resolve, in search order, as
  `%Root{}` rows: absolute path, whether it exists, and how many memories
  it holds.

  The same rows `search/2` reports on its result, for the times you want the
  resolution checked without running a query at all. Both come from one
  function in the CLI, so the two cannot drift.
  """
  @spec roots(keyword()) :: [Root.t()]
  def roots(opts \\ []) do
    opts = Keyword.validate!(opts, dirs: [], cd: nil)
    %{"roots" => roots} = json!("roots", [], [], opts)
    Enum.map(roots, &Root.from_json!/1)
  end

  @doc "One memory by slug, with no ranking fields (`bm25`/`score` nil)."
  @spec show(String.t(), keyword()) :: Hit.t()
  def show(slug, opts \\ []) do
    opts = Keyword.validate!(opts, dirs: [], cd: nil)
    Hit.from_json!(json!("show", [], [slug], opts))
  end

  @doc "Memories whose `based_on` files have moved since they were validated."
  @spec stale(keyword()) :: [Row.t()]
  def stale(opts \\ []), do: rows("stale", [], opts)

  @doc "Memories whose newest validation says the claim did not hold."
  @spec refuted(keyword()) :: [Row.t()]
  def refuted(opts \\ []), do: rows("refuted", [], opts)

  @doc """
  Memories with no validation receipt inside `days:` (the CLI's own
  default is 180): the ones nobody has re-run the proof for.
  """
  @spec unchecked(keyword()) :: [Row.t()]
  def unchecked(opts \\ []) do
    opts = Keyword.validate!(opts, days: nil, dirs: [], cd: nil)
    rows("unchecked", option("--days", opts[:days]), opts)
  end

  @doc """
  Lint every discoverable memory: `%{diagnostics: [%Diagnostic{}], errors:,
  checked:}`. A lint error is an exit 1 the CLI reports in the report
  itself, so a failing lint returns diagnostics rather than raising.
  """
  @spec lint(keyword()) :: lint_report()
  def lint(opts \\ []) do
    opts = Keyword.validate!(opts, dirs: [], cd: nil)
    report = json!("lint", [], [], opts, [0, 1])

    %{
      diagnostics: Enum.map(Map.fetch!(report, "diagnostics"), &Diagnostic.from_json!/1),
      errors: Map.fetch!(report, "errors"),
      checked: Map.fetch!(report, "checked")
    }
  end

  @doc """
  Write a memory: `Memories.remember("nix-rebuild-cascade", "one-line
  tldr", body: markdown, topic: [:nix], handle: ~w(nix-dag),
  based_on: ["packages/nix-dag/src/rank.rs"], validated: ...)`.

  `tldr` is the whole memory a reader gets for free, so it states the
  finding, not the topic. `based_on:` paths are hashed now and re-hashed
  by `validate/2`, which is what makes staleness detectable at all;
  `prior:` is written once at birth and never edited; `supersedes:`
  replaces a memory that was wrong without deleting it. `scope:` is
  `"shared"` (the default) or `"user:<name>"` for a memory that is only
  yours; a hit carries it back, so a caller can tell the two apart.

  Nothing is injected at session start: a memory reaches a model because it
  searched (`CONTRACT.md`, "Nothing is injected unasked"), so there is no
  `always:` field to set.

  The body goes to the CLI on stdin, so it can be a whole reference page.
  """
  @spec remember(String.t(), String.t(), keyword()) :: :ok
  def remember(slug, tldr, opts \\ []) do
    opts =
      Keyword.validate!(opts,
        body: nil,
        genre: nil,
        topic: [],
        handle: [],
        prior: nil,
        related: [],
        based_on: [],
        scope: nil,
        dirs: [],
        cd: nil
      )

    flags =
      option("--tldr", tldr) ++
        option("--genre", opts[:genre]) ++
        options("--topic", opts[:topic]) ++
        options("--handle", opts[:handle]) ++
        option("--prior", opts[:prior]) ++
        options("--related", opts[:related]) ++
        options("--based-on", opts[:based_on]) ++
        option("--scope", opts[:scope])

    write!("remember", flags, [slug], opts)
  end

  @doc """
  Record that a memory was re-checked: `Memories.validate("slug",
  by: "claude-opus-5", how: "the exact command", ok: false)`.

  `how:` is required because a validation nobody can re-run is only a
  date. `ok: false` records the check that failed rather than hiding it,
  which is how a memory becomes refuted. Validating also re-hashes every
  `based_on` path, so it clears staleness.
  """
  @spec validate(String.t(), keyword()) :: :ok
  def validate(slug, opts \\ []) do
    opts = Keyword.validate!(opts, [:by, :how, ok: true, dirs: [], cd: nil])

    flags =
      option("--by", Keyword.fetch!(opts, :by)) ++
        option("--how", Keyword.fetch!(opts, :how)) ++
        switch("--not-ok", not opts[:ok])

    write!("validate", flags, [slug], opts)
  end

  @doc """
  Mark a memory wrong: `Memories.refute("slug", by: "claude-opus-5",
  how: "the command that disagreed", instead: "new-slug")`.

  A refuted memory drops out of `search/2` (unless `all: true`) and stays
  on disk, so the correction has something to point at. `instead:` names
  the memory that replaces it.
  """
  @spec refute(String.t(), keyword()) :: :ok
  def refute(slug, opts \\ []) do
    opts = Keyword.validate!(opts, [:by, :how, instead: nil, dirs: [], cd: nil])

    flags =
      option("--by", Keyword.fetch!(opts, :by)) ++
        option("--how", Keyword.fetch!(opts, :how)) ++
        option("--instead", opts[:instead])

    write!("refute", flags, [slug], opts)
  end

  @doc false
  @spec memories_bin!() :: String.t()
  def memories_bin! do
    System.get_env("MEMORIES_BIN") || System.find_executable("memories") ||
      raise "memories binary not found: install memories on PATH or set MEMORIES_BIN " <>
              "(build it with `nix build .#memories`)"
  end

  @spec grow([Hit.t()], non_neg_integer(), String.t() | nil) :: [Hit.t()]
  defp grow(hits, depth, _cd) when depth <= 0, do: hits

  defp grow(hits, depth, cd) do
    have = MapSet.new(hits, & &1.slug)

    wanted =
      hits
      |> Enum.flat_map(fn hit -> Enum.map(hit.related, &{&1, memories_dir(hit)}) end)
      |> Enum.reject(fn {slug, _dir} -> MapSet.member?(have, slug) end)
      |> Enum.uniq_by(fn {slug, _dir} -> slug end)

    case wanted do
      [] -> hits
      _ -> grow(hits ++ Enum.map(wanted, &neighbour(&1, cd)), depth - 1, cd)
    end
  end

  @spec neighbour({String.t(), String.t()}, String.t() | nil) :: Hit.t()
  defp neighbour({slug, dir}, cd), do: show(slug, dirs: [dir], cd: cd)

  # The `.memories` root a hit came from, which is what `--dir` names --
  # never `Path.dirname(hit.path)`, because a memory may sit one level deep
  # in a grouping subdirectory (`.memories/cas/cas-gc-proof.md`) and that
  # subdirectory is not a root: passing it would hide every sibling group.
  # A hit's `root` is the project directory in the contract's example while
  # the `roots` array holds `.memories` paths, so either spelling of the
  # same directory is accepted rather than guessing which one arrives.
  @spec memories_dir(Hit.t()) :: String.t()
  defp memories_dir(%Hit{root: root}) do
    if Path.basename(root) == ".memories", do: root, else: Path.join(root, ".memories")
  end

  @spec rows(String.t(), [String.t()], keyword()) :: [Row.t()]
  defp rows(subcommand, flags, opts) do
    opts = Keyword.validate!(opts, days: nil, dirs: [], cd: nil)
    %{"rows" => rows} = json!(subcommand, flags, [], opts)
    Enum.map(rows, &Row.from_json!/1)
  end

  # Every `--dir` and `--json` lead, then the subcommand's own flags, then
  # `--` and the positionals: a slug or query starting with `-` is data.
  @spec json!(String.t(), [String.t()], [String.t()], keyword(), [non_neg_integer()]) :: map()
  defp json!(subcommand, flags, positionals, opts, ok_codes \\ [0]) do
    args = argv(subcommand, ["--json"] ++ flags, positionals, opts)
    out = run!(args, nil, opts, ok_codes)

    case JSON.decode(out) do
      {:ok, decoded} ->
        decoded

      {:error, reason} ->
        raise "memories #{subcommand}: expected JSON (#{inspect(reason)}): #{out}"
    end
  end

  # The write subcommands print for a terminal, and the contract does not
  # specify a machine shape for them, so nothing is decoded: the exit
  # status is the outcome and `show/2` reads back what landed.
  @spec write!(String.t(), [String.t()], [String.t()], keyword()) :: :ok
  defp write!(subcommand, flags, positionals, opts) do
    run!(argv(subcommand, flags, positionals, opts), opts[:body], opts, [0])
    :ok
  end

  @spec argv(String.t(), [String.t()], [String.t()], keyword()) :: [String.t()]
  defp argv(subcommand, flags, positionals, opts) do
    dirs = dir_flags(Keyword.get(opts, :dirs, []))
    separator = if positionals == [], do: [], else: ["--"]
    [subcommand] ++ dirs ++ flags ++ separator ++ positionals
  end

  # A root set is plural, so `dirs:` is a list and only a list: a bare
  # string refused here beats two spellings of the same option, one of
  # which nobody tests.
  @spec dir_flags(term()) :: [String.t()]
  defp dir_flags(dirs) when is_list(dirs) do
    Enum.flat_map(dirs, fn
      dir when is_binary(dir) ->
        ["--dir", dir]

      other ->
        raise ArgumentError,
              "dirs: expects .memories paths, got #{inspect(other)} in the list"
    end)
  end

  defp dir_flags(other) do
    raise ArgumentError,
          "dirs: expects a list of .memories paths (one directory is " <>
            "[#{inspect(other)}]), got #{inspect(other)}"
  end

  @spec run!([String.t()], String.t() | nil, keyword(), [non_neg_integer()]) :: String.t()
  defp run!(args, body, opts, ok_codes) do
    bin = memories_bin!()
    cmd_opts = [stderr_to_stdout: true] ++ cd(opts)

    {out, code} =
      case body do
        nil -> Cmd.run(bin, args, cmd_opts)
        body -> run_with_stdin(bin, args, body, cmd_opts)
      end

    if code in ok_codes do
      out
    else
      raise "memories #{Enum.join(args, " ")} exited #{code}: #{out}"
    end
  end

  # `remember` takes its body on stdin, and a port's stdin is a pipe the
  # BEAM never closes and cannot half-close, so the body goes through a
  # temp file the shell redirects from. Still `Cmd.run/3`, so the launch-dir
  # default and its cwd checks (#3902, #3979) hold: it spawns an inner `sh`
  # whose redirect replaces the outer `</dev/null`. The path travels in the
  # environment, so no quoting of it can reach the shell.
  @spec run_with_stdin(String.t(), [String.t()], String.t(), keyword()) ::
          {String.t(), non_neg_integer()}
  defp run_with_stdin(bin, args, body, cmd_opts) do
    path = Path.join(System.tmp_dir!(), "memories-body-#{System.unique_integer([:positive])}")
    File.write!(path, body)

    try do
      Cmd.run(
        "/bin/sh",
        ["-c", ~S(exec "$0" "$@" <"$IX_MEMORIES_BODY"), bin | args],
        cmd_opts ++ [env: [{"IX_MEMORIES_BODY", path}]]
      )
    after
      File.rm(path)
    end
  end

  @spec cd(keyword()) :: keyword()
  defp cd(opts) do
    case Keyword.get(opts, :cd) do
      nil -> []
      dir -> [cd: dir]
    end
  end

  @spec option(String.t(), term()) :: [String.t()]
  defp option(_flag, nil), do: []
  defp option(flag, value), do: [flag, to_string(value)]

  @spec options(String.t(), term()) :: [String.t()]
  defp options(flag, values) do
    Enum.flat_map(List.wrap(values), &[flag, to_string(&1)])
  end

  @spec switch(String.t(), boolean()) :: [String.t()]
  defp switch(_flag, false), do: []
  defp switch(flag, true), do: [flag]
end
