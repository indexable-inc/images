defmodule SymphonyElixirWeb.Markdown do
  @moduledoc """
  Render markdown source to sanitized, dashboard-safe HTML.

  Skill bodies and codex message/reasoning text are authored as
  markdown; the dashboard used to print them verbatim in a `<pre>`, so
  headings, lists, fenced code, and emphasis showed as raw syntax. This
  lowers that source to HTML once at render time.

  Earmark defaults to `escape: true`, so raw HTML in the source is
  neutralized; the output is still run through
  `HtmlSanitizeEx.markdown_html/1` because the dashboard is served
  read-only on a public host and the codex transcript text is
  agent-authored.
  """

  @doc """
  Lower a markdown string to a `{:safe, iodata}` tuple HEEx renders
  without re-escaping. `nil` and blank input render as empty so callers
  can pipe straight from optional fields.
  """
  # The raw/1 call below is the point of this module; sobelow reports it
  # as XSS.Raw (Low Confidence). The HTML it wraps is Earmark output
  # (escape: true) passed through HtmlSanitizeEx.markdown_html/1 first, so
  # the sink is sanitized. sobelow runs reporting-only per .sobelow-conf,
  # so this stays a documented, expected finding rather than a skip
  # annotation.
  @spec to_html(String.t() | nil) :: Phoenix.HTML.safe()
  def to_html(nil), do: Phoenix.HTML.raw("")

  def to_html(source) when is_binary(source) do
    case String.trim(source) do
      "" ->
        Phoenix.HTML.raw("")

      _ ->
        source
        |> Earmark.as_html!(compact_output: true)
        |> HtmlSanitizeEx.markdown_html()
        |> Phoenix.HTML.raw()
    end
  end
end
