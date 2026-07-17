defmodule IxMcp.Api do
  @moduledoc """
  The discovery surface, in-language (the workspace prelude aliases it):

      Api.api()                 # every function of the bundled surface, one row each
      Api.api("tail")           # rows whose name or summary mentions "tail"
      Api.help(Jobs)            # a module's full doc
      Api.help(Jobs, :tail)     # one function's full doc

  Docs come live from `Code.fetch_docs/1`, so what you read is what is
  actually loaded -- the same guarantee the Python `api()` gave with its
  provenance column.
  """

  @surface [IxMcp.Jobs, IxMcp.Api, IxMcp.Kernel, IxMcp.Workspace, IxMcp.Checkpoint, IxMcp.Reader]

  @type row :: %{
          module: module(),
          name: atom(),
          arity: non_neg_integer(),
          signature: String.t(),
          summary: String.t()
        }

  @doc "Rows for the bundled surface, optionally filtered by substring on name/signature/summary."
  @spec api(String.t() | nil) :: [row()]
  def api(filter \\ nil) do
    rows = Enum.flat_map(@surface, &module_rows/1)

    case filter do
      nil ->
        rows

      needle ->
        down = String.downcase(needle)

        Enum.filter(rows, fn row ->
          String.contains?(String.downcase("#{row.name} #{row.signature} #{row.summary}"), down)
        end)
    end
  end

  @doc "Render `api/1` rows as an aligned text table (what the tools surface prints)."
  @spec render([row()]) :: String.t()
  def render(rows) do
    Enum.map_join(rows, "\n", fn row ->
      mod = row.module |> inspect() |> String.replace_prefix("IxMcp.", "")
      String.pad_trailing("#{mod}.#{row.signature}", 46) <> " " <> row.summary
    end)
  end

  @doc "Full documentation for a module."
  @spec help(module()) :: String.t()
  def help(module) do
    case Code.fetch_docs(module) do
      {:docs_v1, _, _, _, %{"en" => moduledoc}, _, _} ->
        "# #{inspect(module)}\n\n#{moduledoc}\n\n" <> render(module_rows(module))

      {:docs_v1, _, _, _, _, _, _} ->
        "#{inspect(module)}: no module doc\n\n" <> render(module_rows(module))

      {:error, reason} ->
        "no docs for #{inspect(module)}: #{inspect(reason)}"
    end
  end

  @doc "Full documentation for one function of a module."
  @spec help(module(), atom()) :: String.t()
  def help(module, function) do
    case Code.fetch_docs(module) do
      {:docs_v1, _, _, _, _, _, docs} ->
        docs
        |> Enum.filter(&match?({{:function, ^function, _}, _, _, _, _}, &1))
        |> case do
          [] ->
            "#{inspect(module)}.#{function}: not found"

          matches ->
            Enum.map_join(matches, "\n\n", fn {{:function, _, _}, _, [signature | _], doc, _} ->
              "#{inspect(module)}.#{signature}\n\n#{doc_text(doc)}"
            end)
        end

      {:error, reason} ->
        "no docs for #{inspect(module)}: #{inspect(reason)}"
    end
  end

  defp module_rows(module) do
    case Code.fetch_docs(module) do
      {:docs_v1, _, _, _, _, _, docs} ->
        for {{:function, name, arity}, _anno, signatures, doc, _meta} <- docs,
            doc != :hidden do
          %{
            module: module,
            name: name,
            arity: arity,
            signature: List.first(signatures) || "#{name}/#{arity}",
            summary: doc |> doc_text() |> first_sentence()
          }
        end

      {:error, _} ->
        []
    end
  end

  defp doc_text(%{"en" => text}), do: text
  defp doc_text(_), do: ""

  defp first_sentence(text) do
    text
    |> String.split("\n", parts: 2)
    |> List.first()
    |> String.slice(0, 120)
  end
end
