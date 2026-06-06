defmodule SymphonyElixir.Skill do
  @moduledoc """
  A skill is a markdown file under `skills/` whose YAML frontmatter declares
  the codex runtime envelope (model, reasoning effort, sandbox, approval
  policy, tools) and whose body is the system prompt for that node type.

  Example `skills/implement.md`:

      ---
      codex_model: gpt-5-codex
      reasoning_effort: medium
      sandbox: workspace-write
      approval_policy: never
      tools: [linear_graphql]
      ---

      Take the Linear ticket in $INPUT. Land the change. Open a PR per
      the PR-submission partial. On block, drop the symphony label and
      exit.

  The body is the lever for improving the agent without code changes.

  ## Partials

  A skill body can include a shared partial by writing
  `{{partial:<name>}}` on its own line (or anywhere; the token is
  replaced inline). The loader resolves the name to
  `skills/_partials/<name>.md` and substitutes the partial's bytes. Use
  partials for cross-skill contracts like the Linear-issue markdown
  template.

  Partials live alongside the skill files under `_partials/`. They are
  not skills themselves: the catalog ignores files in `_partials/`
  because it globs `*.md` non-recursively against the skills directory.

  Partial files are NOT hot-reloaded on their own. To pick up a partial
  edit, also touch the skill files that reference it.

  ### Expansion algorithm

  `expand_partials` runs to a fixpoint with a seen-set:

  - The first `{{partial:NAME}}` token for a given NAME loads
    `_partials/NAME.md`, substitutes its bytes, and marks NAME as seen.
  - Any later `{{partial:NAME}}` whose NAME is already in the seen-set is
    dropped (replaced with empty string). This covers the self-reference
    case: a partial that documents its own token name on a prose line will
    have that token removed rather than cause a missing-partial error or
    an infinite loop. It also deduplicates repeated includes of the same
    partial in one skill body.
  - Tokens inside an inlined partial body are themselves expanded in the
    next iteration, so nested partials work: partial A may include partial
    B, which expands on the subsequent pass.
  - Iteration stops when no `{{partial:...}}` tokens remain.
  - A genuinely-missing partial (NAME not yet seen AND
    `_partials/NAME.md` absent on disk) is a load error
    (`{:error, {:missing_partial, name, reason}}`); the catalog refuses
    to publish a skill with an unresolvable include.

  The seen-set is what prevented the prod outage: partial files that
  contained a literal self-reference token in their prose header would
  leave residual tokens in the catalog body, which `Prompt.build` then
  re-scanned and hard-errored on. The fixpoint + seen-set makes the
  catalog body genuinely token-free regardless of partial prose content.

  ## Reasoning effort

  Optional. Accepts `none`, `minimal`, `low`, `medium`, `high`, or
  `xhigh`, mirroring the codex app-server `ReasoningEffort` enum. When
  set, the runtime passes it as the `effort` parameter on `turn/start`
  so the codex session uses that reasoning budget. When absent, codex
  falls back to its built-in default for the model.
  """

  @enforce_keys [
    :name,
    :path,
    :codex_model,
    :reasoning_effort,
    :sandbox,
    :approval_policy,
    :tools,
    :body,
    :body_hash
  ]
  defstruct [
    :name,
    :path,
    :codex_model,
    :reasoning_effort,
    :sandbox,
    :approval_policy,
    :tools,
    :body,
    :body_hash
  ]

  @type reasoning_effort :: nil | String.t()
  @type t :: %__MODULE__{
          name: String.t(),
          path: Path.t(),
          codex_model: String.t(),
          reasoning_effort: reasoning_effort(),
          sandbox: String.t(),
          approval_policy: String.t(),
          tools: [String.t()],
          body: String.t(),
          body_hash: binary()
        }

  @reasoning_efforts ~w(none minimal low medium high xhigh)

  @spec load(Path.t()) :: {:ok, t()} | {:error, term()}
  def load(path) when is_binary(path) do
    with {:ok, raw} <- File.read(path),
         {:ok, frontmatter_raw, body} <- split_frontmatter(raw),
         {:ok, frontmatter} <- decode_yaml(frontmatter_raw),
         {:ok, body} <- expand_partials(body, partials_dir(path)),
         {:ok, parsed} <- from_parts(frontmatter, body, path) do
      {:ok, %{parsed | body_hash: :crypto.hash(:sha256, raw)}}
    end
  end

  @spec from_parts(map(), String.t(), Path.t()) :: {:ok, t()} | {:error, term()}
  defp from_parts(frontmatter, body, path)
       when is_map(frontmatter) and is_binary(body) and is_binary(path) do
    with {:ok, codex_model} <- fetch_string(frontmatter, "codex_model"),
         {:ok, reasoning_effort} <- fetch_reasoning_effort(frontmatter),
         {:ok, sandbox} <- fetch_string(frontmatter, "sandbox"),
         {:ok, approval_policy} <- fetch_string(frontmatter, "approval_policy"),
         {:ok, tools} <- fetch_string_list(frontmatter, "tools") do
      name =
        path
        |> Path.basename(".md")

      {:ok,
       %__MODULE__{
         name: name,
         path: path,
         codex_model: codex_model,
         reasoning_effort: reasoning_effort,
         sandbox: sandbox,
         approval_policy: approval_policy,
         tools: tools,
         body: body,
         body_hash: <<>>
       }}
    end
  end

  defp split_frontmatter(raw) do
    case String.split(raw, ~r/^---\s*\n/m, parts: 3) do
      ["", frontmatter, body] -> {:ok, frontmatter, String.trim_leading(body)}
      _ -> {:error, :missing_frontmatter}
    end
  end

  defp decode_yaml(raw) do
    case YamlElixir.read_from_string(raw) do
      {:ok, decoded} when is_map(decoded) -> {:ok, decoded}
      {:ok, _other} -> {:error, :invalid_frontmatter}
      {:error, reason} -> {:error, {:yaml_decode_failed, reason}}
    end
  end

  defp fetch_string(map, key) do
    case Map.get(map, key) do
      value when is_binary(value) ->
        case String.trim(value) do
          "" -> {:error, {:missing_field, key}}
          trimmed -> {:ok, trimmed}
        end

      _ ->
        {:error, {:missing_field, key}}
    end
  end

  defp fetch_string_list(map, key) do
    case Map.get(map, key, []) do
      list when is_list(list) ->
        if Enum.all?(list, &is_binary/1) do
          {:ok, list}
        else
          {:error, {:invalid_field, key, list}}
        end

      _ ->
        {:error, {:invalid_field, key}}
    end
  end

  defp fetch_reasoning_effort(map) do
    case Map.get(map, "reasoning_effort") do
      nil ->
        {:ok, nil}

      value when is_binary(value) ->
        normalized = value |> String.trim() |> String.downcase()

        cond do
          normalized == "" -> {:ok, nil}
          normalized in @reasoning_efforts -> {:ok, normalized}
          true -> {:error, {:invalid_reasoning_effort, value}}
        end

      other ->
        {:error, {:invalid_reasoning_effort, other}}
    end
  end

  defp partials_dir(skill_path) do
    skill_path
    |> Path.dirname()
    |> Path.join("_partials")
  end

  # Expand every `{{partial:<name>}}` occurrence with the bytes of
  # `<partials_dir>/<name>.md`, running to a fixpoint with a seen-set so
  # each named partial is inlined at most once.
  #
  # Names are limited to `[A-Za-z0-9_-]+` so the resolver cannot escape
  # the partials directory.
  #
  # Resolving the leftmost token one at a time is what makes "at most
  # once" exact. The first occurrence of a name is replaced with its
  # partial bytes and the name is marked seen; every later occurrence of
  # that name is dropped. A later occurrence is either a repeat include in
  # the skill body or the self-reference token a partial carries in its
  # own prose. Dropping that self-reference is what prevented the prod
  # outage: under the old single-pass loader the token survived in the
  # catalog body and `Prompt.build` then re-scanned and hard-errored on
  # it. Tokens carried in from an inlined body are resolved on a later
  # pass, so nested partials expand.
  #
  # Each step either marks one new name seen (finite) or removes one
  # token, so the loop terminates. A genuinely-missing partial (NAME not
  # yet seen and the file absent) is a load error so the catalog refuses
  # to publish a half-rendered skill body.
  @partial_token ~r/\{\{partial:([A-Za-z0-9_-]+)\}\}/

  defp expand_partials(body, partials_dir) when is_binary(body) and is_binary(partials_dir) do
    expand_partials_loop(body, partials_dir, [])
  end

  # `seen` lists the partial names already inlined. A plain list keeps the
  # recursive boundary free of MapSet's opaque type, which dialyzer rejects
  # here as a `call_without_opaque` mismatch even with a `MapSet.t()` spec;
  # a skill includes only a handful of partials, so linear membership is
  # irrelevant.
  @spec expand_partials_loop(String.t(), String.t(), [String.t()]) ::
          {:ok, String.t()} | {:error, term()}
  defp expand_partials_loop(body, partials_dir, seen) do
    case Regex.run(@partial_token, body) do
      nil ->
        {:ok, body}

      [token, name] ->
        if name in seen do
          expand_partials_loop(replace_first(body, token, ""), partials_dir, seen)
        else
          partial_path = Path.join(partials_dir, name <> ".md")

          case File.read(partial_path) do
            {:ok, contents} ->
              inlined = replace_first(body, token, String.trim_trailing(contents))
              expand_partials_loop(inlined, partials_dir, [name | seen])

            {:error, reason} ->
              {:error, {:missing_partial, name, reason}}
          end
        end
    end
  end

  # Replace only the matched (leftmost) token. `Regex.run` returns the
  # leftmost match, so a literal first-occurrence replace rewrites exactly
  # that token and leaves any later occurrence for a subsequent pass.
  defp replace_first(body, token, replacement) do
    String.replace(body, token, replacement, global: false)
  end
end
