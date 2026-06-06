defmodule Mix.Tasks.PrBody.Check do
  @moduledoc """
  Validates that a pull request body follows the repository template.
  """

  use Mix.Task

  @shortdoc "Validate a pull request body markdown file"

  @sections [
    "#### Context",
    "#### TL;DR",
    "#### Summary",
    "#### Alternatives",
    "#### Test Plan"
  ]

  @impl Mix.Task
  def run(args) do
    {opts, _rest, invalid} = OptionParser.parse(args, strict: [file: :string])

    case {Keyword.fetch(opts, :file), invalid} do
      {{:ok, path}, []} ->
        path
        |> read_body()
        |> validate_body()

      _ ->
        Mix.raise("usage: mix pr_body.check --file /path/to/pr_body.md")
    end
  end

  defp read_body(path) do
    case File.read(path) do
      {:ok, body} -> body
      {:error, reason} -> Mix.raise("failed to read #{path}: #{:file.format_error(reason)}")
    end
  end

  defp validate_body(body) do
    with :ok <- validate_sections_present(body),
         {:ok, sections} <- split_sections(body),
         :ok <- validate_no_template_comments(body),
         :ok <- validate_context(sections),
         :ok <- validate_tldr(sections),
         :ok <- validate_list_section(sections, "#### Summary"),
         :ok <- validate_list_section(sections, "#### Alternatives"),
         :ok <- validate_test_plan(sections) do
      Mix.shell().info("PR body matches the repository template")
    else
      {:error, reason} -> Mix.raise(reason)
    end
  end

  defp validate_sections_present(body) do
    missing = Enum.reject(@sections, &String.contains?(body, &1))

    case missing do
      [] -> :ok
      sections -> {:error, "missing PR body sections: #{Enum.join(sections, ", ")}"}
    end
  end

  defp split_sections(body) do
    positions =
      Enum.map(@sections, fn section ->
        case :binary.match(body, section) do
          {index, _length} -> {section, index}
          :nomatch -> nil
        end
      end)

    if Enum.any?(positions, &is_nil/1) do
      {:error, "missing PR body sections"}
    else
      sections =
        positions
        |> Enum.with_index()
        |> Map.new(fn {{section, start_index}, index} ->
          content_start = start_index + byte_size(section)

          content_end =
            case Enum.at(positions, index + 1) do
              {_next_section, next_start} -> next_start
              nil -> byte_size(body)
            end

          {section, body |> binary_part(content_start, content_end - content_start) |> String.trim()}
        end)

      {:ok, sections}
    end
  end

  defp validate_no_template_comments(body) do
    if String.contains?(body, "<!--") do
      {:error, "PR body still contains template comments"}
    else
      :ok
    end
  end

  defp validate_context(sections) do
    text = Map.fetch!(sections, "#### Context")

    cond do
      text == "" -> {:error, "Context section must not be empty"}
      String.length(text) > 240 -> {:error, "Context section must be 240 characters or fewer"}
      true -> :ok
    end
  end

  defp validate_tldr(sections) do
    text = Map.fetch!(sections, "#### TL;DR")

    cond do
      text == "" -> {:error, "TL;DR section must not be empty"}
      String.length(String.trim(text, "*")) > 120 -> {:error, "TL;DR section must be 120 characters or fewer"}
      true -> :ok
    end
  end

  defp validate_list_section(sections, section) do
    lines =
      sections
      |> Map.fetch!(section)
      |> list_lines()

    if lines == [] do
      {:error, "#{String.trim_leading(section, "# ")} section must include at least one list item"}
    else
      :ok
    end
  end

  defp validate_test_plan(sections) do
    lines =
      sections
      |> Map.fetch!("#### Test Plan")
      |> list_lines()

    cond do
      lines == [] -> {:error, "Test Plan section must include at least one checkbox"}
      Enum.all?(lines, &String.starts_with?(&1, ["- [ ] ", "- [x] ", "- [X] "])) -> :ok
      true -> {:error, "Test Plan items must be markdown checkboxes"}
    end
  end

  defp list_lines(text) do
    text
    |> String.split("\n")
    |> Enum.map(&String.trim/1)
    |> Enum.filter(&String.starts_with?(&1, "- "))
  end
end
