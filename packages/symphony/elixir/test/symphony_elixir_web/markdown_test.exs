defmodule SymphonyElixirWeb.MarkdownTest do
  @moduledoc """
  The dashboard lowers skill bodies and codex transcript text from
  markdown to sanitized HTML. These guard the render-and-sanitize
  contract: structural markdown becomes HTML, blank input stays empty,
  and script injection is scrubbed before it reaches a `{:safe, _}`.
  """

  use ExUnit.Case, async: true

  alias SymphonyElixirWeb.Markdown

  defp render(source) do
    {:safe, iodata} = Markdown.to_html(source)
    IO.iodata_to_binary(iodata)
  end

  test "renders headings, emphasis, lists, and inline code" do
    html =
      render("""
      # Sub tickets

      Split **the work** into `tasks`:

      - first
      - second
      """)

    assert html =~ "<h1>"
    assert html =~ "Sub tickets"
    assert html =~ "<strong>the work</strong>"
    assert html =~ ~r{<code[^>]*>tasks</code>}
    assert html =~ "<li>first</li>"
  end

  test "renders fenced code blocks" do
    html =
      render("""
      ```
      mix deps.get
      ```
      """)

    assert html =~ "<pre>"
    assert html =~ "mix deps.get"
  end

  test "blank and nil input render as empty safe html" do
    assert Markdown.to_html(nil) == {:safe, ""}
    assert Markdown.to_html("") == {:safe, ""}
    assert Markdown.to_html("   \n  ") == {:safe, ""}
  end

  test "neutralizes raw html so transcript text cannot inject" do
    html = render("hello <script>alert('x')</script> world")

    # Earmark escapes raw html by default and the sanitizer is a second
    # line of defense, so no executable script element survives.
    refute html =~ "<script"
    assert html =~ "&lt;script&gt;"
    assert html =~ "hello"
    assert html =~ "world"
  end
end
