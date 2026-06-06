defmodule SymphonyElixirWeb.StaticAssetController do
  @moduledoc """
  Serves the JS bundles Phoenix LiveView needs, read directly from the
  dep checkout. Avoids a build pipeline for v0.
  """

  use Phoenix.Controller, formats: []

  def phoenix(conn, _params), do: send_dep_js(conn, :phoenix, "priv/static/phoenix.js")
  def phoenix_html(conn, _params), do: send_dep_js(conn, :phoenix_html, "priv/static/phoenix_html.js")
  def phoenix_live_view(conn, _params), do: send_dep_js(conn, :phoenix_live_view, "priv/static/phoenix_live_view.js")

  defp send_dep_js(conn, app, relative_path) do
    priv = :code.priv_dir(app) |> to_string()
    full = Path.join(Path.dirname(priv), relative_path)

    conn
    |> put_resp_content_type("application/javascript")
    |> put_resp_header("cache-control", "public, max-age=3600")
    |> send_file(200, full)
  end
end
