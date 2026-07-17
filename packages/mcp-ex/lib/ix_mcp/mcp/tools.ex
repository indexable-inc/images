defmodule IxMcp.MCP.Tools do
  @moduledoc """
  The MCP tool surface. Every tool returns `{:ok, text}` or `{:error, text}`;
  the transport wraps that in the MCP content envelope.
  """

  alias IxMcp.Jobs

  @budget_cap 120

  @spec list() :: [map()]
  def list do
    [
      %{
        "name" => "elixir_exec",
        "description" => """
        Run Elixir on the shared persistent workspace. Waits up to `budget`
        seconds; if the code is still running it keeps going in the background
        as a job and this returns a job handle. Bindings persist across calls:
        variables, aliases, imports, and modules you define stay defined.
        Job control is in-language via the `Jobs` module (aliased in every
        cell): `Jobs.tail("ab12", 20)`, `Jobs.await("ab12")`,
        `Jobs.cancel("ab12")`, `Jobs.history()`. Each cell runs in its own
        BEAM process, so a blocking cell never delays other jobs or this
        server.
        """,
        "inputSchema" => %{
          "type" => "object",
          "properties" => %{
            "code" => %{"type" => "string", "description" => "Elixir source to evaluate"},
            "budget" => %{
              "type" => "number",
              "description" =>
                "Seconds to wait before backgrounding the run (cap: #{@budget_cap}s)",
              "default" => 15
            },
            "intent" => %{
              "type" => "string",
              "description" => "Short human description of what this run does (titles the run)"
            }
          },
          "required" => ["code", "intent"]
        }
      },
      %{
        "name" => "session_set_name",
        "description" =>
          "Name this connection's session. Call before acting tools; the name labels every run this server records.",
        "inputSchema" => %{
          "type" => "object",
          "properties" => %{"name" => %{"type" => "string", "minLength" => 3, "maxLength" => 80}},
          "required" => ["name"]
        }
      },
      %{
        "name" => "topic_set",
        "description" =>
          "Set the current topic. Runs fold under the topic inside the session; change it when work moves to a new phase.",
        "inputSchema" => %{
          "type" => "object",
          "properties" => %{"topic" => %{"type" => "string", "minLength" => 3, "maxLength" => 80}},
          "required" => ["topic"]
        }
      },
      %{
        "name" => "read",
        "description" => """
        Read a file (or a workspace value) into your context. `target` is read
        as a file when it names one on disk; otherwise it is evaluated as an
        Elixir expression against the shared workspace, exactly like a cell
        (e.g. `Jobs.output("ab12")`, or a variable you bound earlier). An
        expression whose value is a string naming an existing file reads that
        file too. Pass `start`/`end` for a 1-based inclusive line range.
        """,
        "inputSchema" => %{
          "type" => "object",
          "properties" => %{
            "target" => %{"type" => "string"},
            "start" => %{"type" => "integer", "minimum" => 1},
            "end" => %{"type" => "integer", "minimum" => 1}
          },
          "required" => ["target"]
        }
      },
      %{
        "name" => "kernel_trace",
        "description" => """
        Dump the current stack of every running job's processes (and core
        server processes), taken from outside with Process.info/2. Works no
        matter what any cell is doing -- no cell can block this server -- so
        use it to see WHERE a slow job is stuck, then cancel or await it.
        """,
        "inputSchema" => %{"type" => "object", "properties" => %{}}
      },
      %{
        "name" => "kernel_restart",
        "description" => """
        Restart the evaluator on purpose: cancel every running job (their OS
        subprocess trees die with them), restart the workspace process, and
        restore bindings from the in-VM checkpoint. Scoped to this server
        only. Bindings survive; running jobs do not.
        """,
        "inputSchema" => %{"type" => "object", "properties" => %{}}
      }
    ]
  end

  @spec call(String.t(), map()) :: {:ok, String.t()} | {:error, String.t()}
  def call("elixir_exec", %{"code" => code} = args) when is_binary(code) do
    budget = args |> Map.get("budget", 15) |> clamp_budget()
    intent = Map.get(args, "intent")

    {summary, output} = Jobs.run(code, budget: budget, intent: intent)
    {:ok, render_run(summary, output)}
  end

  def call("elixir_exec", _args), do: {:error, "elixir_exec requires string `code`"}

  def call("session_set_name", %{"name" => name}) when is_binary(name) do
    :ok = IxMcp.Session.set_name(name)
    {:ok, "session named: #{name}"}
  end

  def call("session_set_name", _args), do: {:error, "session_set_name requires string `name`"}

  def call("topic_set", %{"topic" => topic}) when is_binary(topic) do
    :ok = IxMcp.Session.set_topic(topic)
    {:ok, "topic set: #{topic}"}
  end

  def call("topic_set", _args), do: {:error, "topic_set requires string `topic`"}

  def call("read", %{"target" => target} = args) when is_binary(target) do
    IxMcp.Reader.read(target, Map.get(args, "start"), Map.get(args, "end"))
  end

  def call("read", _args), do: {:error, "read requires string `target`"}

  def call("kernel_trace", _args), do: {:ok, IxMcp.Kernel.trace()}

  def call("kernel_restart", _args) do
    report = IxMcp.Kernel.restart()

    {:ok,
     "evaluator restarted: cancelled jobs #{inspect(report.jobs_cancelled)}, " <>
       "#{report.bindings_restored} bindings restored"}
  end

  def call(other, _args), do: {:error, "unknown tool: #{other}"}

  defp clamp_budget(budget) when is_number(budget) and budget > 0, do: min(budget, @budget_cap)
  defp clamp_budget(_), do: 15

  defp render_run(summary, output) do
    header =
      JSON.encode!(%{
        "job" => summary.id,
        "status" => Atom.to_string(summary.status),
        "running" => summary.running,
        "elapsed_s" => Float.round(summary.elapsed_s, 2)
      })

    sections =
      [header] ++
        diagnostics_section(summary.diagnostics) ++
        output_section(output) ++
        result_section(summary)

    Enum.join(sections, "\n")
  end

  defp diagnostics_section([]), do: []
  defp diagnostics_section(diags), do: Enum.map(diags, &("-- " <> &1))

  defp output_section(""), do: []
  defp output_section(output), do: [String.trim_trailing(output, "\n")]

  defp result_section(%{running: true}) do
    [
      "job still running; page it with Jobs.tail(id, n) / await it with Jobs.await(id) in a later elixir_exec call"
    ]
  end

  defp result_section(%{status: :done, result: rendered}), do: ["=> " <> rendered]
  defp result_section(%{result: nil}), do: []
  defp result_section(%{result: rendered}), do: [rendered]
end
