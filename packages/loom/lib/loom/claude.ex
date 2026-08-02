defmodule Loom.Claude do
  @moduledoc """
  Argv construction and stream parsing for headless `claude -p` children.

  loom drives children in print mode with `--output-format stream-json`:
  one JSON object per line, an `init` event carrying the `session_id`
  (the handle a wake resumes with), and a terminal `result` event
  carrying the final text. `--verbose` is required by the CLI whenever
  print mode emits stream-json.

  Credential provisioning is the template's job, not this module's: the
  guest is expected to have `ANTHROPIC_API_KEY` available to login
  shells (forks inherit it from the control VM's disk).
  """

  @typedoc "A parsed stream-json line loom acts on; other events pass through as `:other`."
  @type event ::
          {:session, String.t()}
          | {:result, String.t()}
          | {:other, map()}
          | {:not_json, String.t()}

  @doc "The claude executable to run inside the fork (`:loom` env `:claude_bin`)."
  @spec bin() :: String.t()
  def bin, do: Application.get_env(:loom, :claude_bin, "claude")

  @doc "Argv for a fresh child turn on `brief`."
  @spec argv(String.t()) :: [String.t()]
  def argv(brief) when is_binary(brief) do
    [bin(), "-p", brief | common_flags()]
  end

  @doc "Argv resuming session `session_id` with a follow-up instruction."
  @spec resume_argv(String.t(), String.t()) :: [String.t()]
  def resume_argv(session_id, text) when is_binary(session_id) and is_binary(text) do
    [bin(), "-p", "--resume", session_id, text | common_flags()]
  end

  @spec common_flags() :: [String.t()]
  defp common_flags do
    ["--output-format", "stream-json", "--verbose" | extra_args()]
  end

  # Extra claude flags (`:loom` env `:claude_args`). For real workloads
  # the right posture is ["--dangerously-skip-permissions"]: the fork
  # is a disposable VM that IS the sandbox, so in-guest permission
  # prompts protect nothing and stall headless children.
  @spec extra_args() :: [String.t()]
  defp extra_args, do: Application.get_env(:loom, :claude_args, [])

  @doc """
  Classify one output line.

  The stream is line-delimited JSON; anything unparsable (guest boot
  noise, shell banners) is preserved as `{:not_json, line}` rather than
  dropped, so a failing run's evidence survives.
  """
  @spec parse_line(String.t()) :: event()
  def parse_line(line) when is_binary(line) do
    case JSON.decode(line) do
      {:ok, %{"type" => "system", "subtype" => "init", "session_id" => sid}} ->
        {:session, sid}

      {:ok, %{"type" => "result"} = obj} ->
        {:result, Map.get(obj, "result", "")}

      {:ok, obj} when is_map(obj) ->
        {:other, obj}

      _other ->
        {:not_json, line}
    end
  end
end
