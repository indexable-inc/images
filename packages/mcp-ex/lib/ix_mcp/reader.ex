defmodule IxMcp.Reader do
  @moduledoc """
  The `read` tool: fetch a file -- or a workspace value -- into the caller's
  context. `target` is read as a file when it names one on disk; otherwise it
  is evaluated as an Elixir expression against the shared workspace, exactly
  like a cell. An expression whose value is a string naming an existing file
  reads that file too. `start`/`end` select a 1-based inclusive line range.
  """

  alias IxMcp.Evaluator
  alias IxMcp.Jobs

  @eval_budget_s 30

  @spec read(String.t(), pos_integer() | nil, pos_integer() | nil) ::
          {:ok, String.t()} | {:error, String.t()}
  def read(target, first \\ nil, last \\ nil) do
    if File.regular?(target) do
      read_file(target, first, last)
    else
      read_value(target, first, last)
    end
  end

  defp read_value(target, first, last) do
    case Jobs.run(target, budget: @eval_budget_s, intent: "read #{String.slice(target, 0, 40)}") do
      {%{status: :done} = summary, _output} ->
        value_result(summary.id, first, last)

      {%{running: true} = summary, _output} ->
        {:error,
         "expression still evaluating after #{@eval_budget_s}s; page it as job #{summary.id}"}

      {%{result: message}, _output} ->
        {:error, message || "evaluation failed"}
    end
  end

  defp value_result(job_id, first, last) do
    case Jobs.result(job_id) do
      {:ok, value} when is_binary(value) ->
        if File.regular?(value) do
          read_file(value, first, last)
        else
          {:ok, slice_lines(value, first, last)}
        end

      {:ok, value} ->
        {:ok, slice_lines(Evaluator.render(value), first, last)}

      {:error, reason} ->
        {:error, inspect(reason)}
    end
  end

  defp read_file(path, first, last) do
    case File.read(path) do
      {:ok, content} -> {:ok, slice_lines(content, first, last)}
      {:error, reason} -> {:error, "#{path}: #{:file.format_error(reason)}"}
    end
  end

  defp slice_lines(content, nil, nil), do: content

  defp slice_lines(content, first, last) do
    lines = String.split(content, "\n")
    first = max(first || 1, 1)
    last = min(last || length(lines), length(lines))
    lines |> Enum.slice((first - 1)..(last - 1)//1) |> Enum.join("\n")
  end
end
