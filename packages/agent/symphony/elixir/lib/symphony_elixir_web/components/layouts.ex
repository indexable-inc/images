defmodule SymphonyElixirWeb.Layouts do
  @moduledoc "Root and app layouts for the runs dashboard."

  use Phoenix.Component

  alias Phoenix.LiveView.Rendered

  @spec root(map()) :: Rendered.t()
  def root(assigns) do
    assigns = assign(assigns, :csrf_token, Plug.CSRFProtection.get_csrf_token())

    ~H"""
    <!DOCTYPE html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="csrf-token" content={@csrf_token} />
        <title>symphony</title>
        <link
          rel="icon"
          type="image/svg+xml"
          href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'%3E%3Ctext x='50%25' y='50%25' dominant-baseline='central' text-anchor='middle' font-size='90'%3E%F0%9F%8E%B7%3C/text%3E%3C/svg%3E"
        />
        <style>
          :root { --bg: #000000; --fg: #e7e7ea; --muted: #6a6a72; --accent: #e7e7ea; --good: #6ad28a; --bad: #ff6b6b; --warn: #d8a45a; --card: #0a0a0a; --border: #1a1a1c; }
          * { box-sizing: border-box; }
          body { margin: 0; background: var(--bg); color: var(--fg); font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, "Inter", sans-serif; }
          a { color: var(--fg); text-decoration: none; }
          a:hover { text-decoration: underline; }
          main.shell { max-width: none; margin: 0; padding: 32px 32px; }
          header.bar { display: flex; align-items: baseline; justify-content: space-between; margin-bottom: 24px; }
          header.bar h1 { font-size: 18px; font-weight: 600; margin: 0; letter-spacing: -0.01em; display: flex; align-items: baseline; gap: 8px; }
          header.bar h1 .logo-mark { font-size: 18px; line-height: 1; }
          header.bar h1 .logo-bracket { color: var(--muted); font-weight: 500; }
          header.bar h1 .logo-name { color: var(--fg); }
          table.runs { width: 100%; border-collapse: collapse; }
          table.runs th, table.runs td { text-align: left; padding: 10px 12px; border-bottom: 1px solid var(--border); }
          table.runs th { font-weight: 500; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.04em; }
          table.runs tr:hover td { background: var(--card); }
          table.runs td a.ref { font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 12px; color: var(--fg); }
          table.runs td a.ref:hover { text-decoration: underline; }
          table.runs td .ref-empty { color: var(--muted); font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 12px; }
          .pager { display: flex; justify-content: space-between; align-items: center; padding: 10px 12px; color: var(--muted); font-size: 12px; }
          .pager .pages { display: flex; gap: 6px; align-items: center; }
          .pager a, .pager span.page-num { display: inline-block; padding: 4px 10px; border: 1px solid var(--border); color: var(--muted); font-variant-numeric: tabular-nums; }
          .pager a:hover { color: var(--fg); border-color: var(--fg); text-decoration: none; }
          .pager span.page-num.current { color: var(--fg); border-color: var(--fg); }
          .pager a.disabled { opacity: 0.4; pointer-events: none; }
          .pill { display: inline-block; padding: 2px 8px; font-size: 11px; font-weight: 500; letter-spacing: 0.02em; border: 1px solid var(--border); }
          .pill.pending { color: var(--muted); }
          .pill.running { color: var(--fg); border-color: var(--border); }
          .pill.succeeded { color: var(--good); border-color: rgba(106,210,138,0.35); }
          .pill.failed { color: var(--bad); border-color: rgba(255,107,107,0.35); }
          .pill.skipped { color: var(--warn); border-color: rgba(216,164,90,0.35); }
          .card { background: var(--card); border: 1px solid var(--border); padding: 16px; margin-bottom: 16px; }
          .card-header { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; margin-bottom: 10px; }
          .card-header .title { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.04em; }
          .node-grid { display: grid; gap: 10px; grid-template-columns: 1fr; }
          .node-row { display: grid; grid-template-columns: 160px 100px minmax(0, 1fr) 140px; gap: 12px; align-items: center; padding: 8px 10px; background: var(--bg); }
          .mono { font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 12px; }
          .muted { color: var(--muted); }
          .empty { color: var(--muted); padding: 24px; text-align: center; border: 1px dashed var(--border); }
          .actions { display: flex; gap: 8px; align-items: center; }
          .btn { display: inline-block; padding: 6px 12px; background: var(--card); color: var(--fg); border: 1px solid var(--border); cursor: pointer; font-size: 13px; text-decoration: none; }
          .btn:hover { border-color: var(--fg); text-decoration: none; }
          .btn.btn-primary { background: var(--fg); color: var(--bg); border-color: var(--fg); }
          .btn.btn-primary:hover { opacity: 0.9; }
          form.enqueue { display: flex; flex-direction: column; gap: 12px; align-items: stretch; }
          form.enqueue select, form.enqueue input { background: var(--card); color: var(--fg); border: 1px solid var(--border); padding: 6px 10px; font: inherit; }
          form.enqueue .row { display: flex; gap: 12px; align-items: center; width: 100%; }
          form.enqueue label { color: var(--muted); font-size: 12px; min-width: 110px; }
          /* min-width:0 lets the select shrink below its widest <option>; a flex
             item defaults to min-width:auto and otherwise forces the row (and the
             popover) wider than its width, which is the horizontal-scroll overflow. */
          form.enqueue .field-input { flex: 1; min-width: 0; }
          form.enqueue select.field-input { width: 100%; max-width: 100%; text-overflow: ellipsis; }
          form.enqueue .hint { color: var(--muted); font-size: 12px; margin-left: 122px; }
          form.enqueue .field-group { padding: 12px; border: 1px solid var(--border); background: var(--bg); }
          form.enqueue .field-group .field-group-title { color: var(--muted); font-size: 12px; margin-bottom: 10px; text-transform: uppercase; letter-spacing: 0.04em; }
          form.enqueue .submit-row { display: flex; justify-content: flex-end; gap: 8px; }
          .toolbar { display: flex; justify-content: flex-end; margin-bottom: 16px; }
          /* Native popover: a compact launcher anchored top-right so the runs table stays the page. */
          .launcher-popover { position: fixed; inset: 64px 24px auto auto; width: 420px; max-width: calc(100vw - 48px); margin: 0; background: var(--card); color: var(--fg); border: 1px solid var(--border); padding: 20px; box-shadow: 0 12px 32px rgba(0,0,0,0.55); }
          .launcher-popover .launcher-title { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.04em; margin-bottom: 12px; }
          .launcher-popover::backdrop { background: rgba(0,0,0,0.2); }
          nav.tabs { display: flex; gap: 16px; margin-top: 6px; }
          nav.tabs a { color: var(--muted); padding: 4px 0; border-bottom: 1px solid transparent; font-size: 13px; }
          nav.tabs a.active { color: var(--fg); border-bottom-color: var(--fg); }
          nav.tabs a:hover { color: var(--fg); text-decoration: none; }
          .dag-grid { display: grid; gap: 10px; grid-template-columns: 1fr; }
          .dag-row { display: grid; grid-template-columns: 180px minmax(0, 280px) minmax(0, 1fr) 90px; gap: 12px; align-items: center; padding: 10px 12px; background: var(--bg); border: 1px solid var(--border); }
          .dag-row > * { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
          .dag-row .name { font-weight: 500; }
          .dag-row .right-align { text-align: right; }
          .skill-body { white-space: pre-wrap; font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 12px; padding: 12px; background: var(--bg); border: 1px solid var(--border); max-height: 480px; overflow: auto; }
          .kv { display: grid; grid-template-columns: 140px 1fr; gap: 6px 16px; }
          .kv dt { color: var(--muted); font-size: 12px; }
          .kv dd { margin: 0; }
          .dag-diagram { overflow: auto; background: var(--bg); border: 1px solid var(--border); padding: 16px; }
          .dag-diagram svg { display: block; margin: 0 auto; color: var(--muted); }
          .dag-diagram .dnode { fill: var(--card); stroke: var(--border); stroke-width: 1; }
          .dag-diagram .dnode-id { fill: var(--fg); font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 13px; font-weight: 500; }
          .dag-diagram .dnode-skill { fill: var(--muted); font-family: ui-sans-serif, system-ui, sans-serif; font-size: 11px; }
          .dag-diagram .dedge { stroke: var(--muted); stroke-width: 1.5; fill: none; }
          .dag-diagram .darrow { fill: var(--muted); }
          .back-link { display: inline-flex; align-items: center; gap: 4px; color: var(--muted); font-size: 13px; }
          .back-link:hover { color: var(--fg); }
          .codex-sessions { list-style: none; padding: 0; margin: 0; }
          .codex-sessions li { border-top: 1px solid var(--border); margin: 0; }
          .codex-sessions li:last-child { border-bottom: 1px solid var(--border); }
          .codex-sessions li > a { display: block; padding: 10px 0; color: inherit; }
          .codex-sessions li > a:hover { background: var(--bg); text-decoration: none; }
          .codex-sessions .row { display: flex; justify-content: space-between; align-items: baseline; gap: 12px; }
          .codex-sessions .row.top { margin-bottom: 2px; }
          .codex-sessions .row.meta { font-size: 12px; color: var(--muted); gap: 8px; justify-content: flex-start; }
          .codex-sessions .cwd { color: var(--fg); font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 13px; background: none; padding: 0; overflow-wrap: anywhere; }
          .codex-sessions .time { color: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; white-space: nowrap; flex-shrink: 0; }
          .codex-sessions li.live .time { color: var(--fg); font-weight: 500; }
          .codex-sessions li.live .time::before { content: '\25cf  '; color: var(--warn); font-size: 0.6em; vertical-align: 0.2em; }
          .codex-sessions .version { font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; }
          .codex-sessions .preview { margin: 6px 0 0; color: var(--fg); font-size: 13px; line-height: 1.5; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
          .codex-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 16px; }
          .codex-head .state { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--muted); font-variant-numeric: tabular-nums; }
          .codex-head .state .dot { width: 8px; height: 8px; background: var(--muted); }
          .codex-head .state.live .dot { background: var(--warn); box-shadow: 0 0 0 3px rgba(216,164,90,0.2); }
          .codex-head .state.live .label { color: var(--fg); font-weight: 500; }
          .codex-log { padding: 8px 12px; max-height: calc(100vh - 280px); overflow-y: auto; }
          .codex-event { display: inline-block; font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 11px; color: var(--muted); text-transform: lowercase; letter-spacing: 0.04em; background: var(--bg); padding: 2px 6px; margin: 6px 0; }
          .codex-event.tokens { background: none; border: 1px dashed var(--border); }
          .codex-msg { margin: 10px 0; background: var(--bg); border: 1px solid var(--border); padding: 8px 12px; }
          .codex-msg header, .codex-msg summary { color: var(--muted); font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; margin-bottom: 4px; cursor: default; }
          details.codex-msg > summary { cursor: pointer; list-style: none; display: flex; align-items: baseline; gap: 8px; }
          details.codex-msg > summary::-webkit-details-marker { display: none; }
          details.codex-msg > summary::before { content: '\25b8'; display: inline-block; font-size: 0.7em; transition: transform 0.12s ease; }
          details.codex-msg[open] > summary::before { transform: rotate(90deg); }
          details.codex-msg .name { color: var(--fg); text-transform: none; letter-spacing: 0; font-size: 12px; }
          .codex-msg pre { margin: 0; border: none; padding: 0; background: none; color: var(--fg); white-space: pre-wrap; overflow-wrap: anywhere; font-size: 13px; line-height: 1.45; font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; }
          .codex-msg[data-role='user'] { border-color: var(--fg); border-left: 2px solid var(--fg); }
          .codex-msg[data-role='assistant'] { background: var(--card); }
          .codex-msg[data-role='developer'] { opacity: 0.7; }
          .codex-msg[data-role='developer'] pre { color: var(--muted); }
          .codex-msg.reasoning { border-style: dashed; }
          .codex-msg.reasoning pre { color: var(--muted); font-style: italic; }
          .codex-msg.tool-output pre { max-height: 280px; overflow: auto; }
          .codex-msg.unknown { opacity: 0.6; }
          /* Rendered markdown (skill bodies, codex message/reasoning text).
             Resets the pre-wrap/monospace container styling these used to
             inherit so block elements lay out as prose. */
          .markdown { white-space: normal; font-family: ui-sans-serif, system-ui, -apple-system, "Inter", sans-serif; font-size: 13px; line-height: 1.55; color: var(--fg); overflow-wrap: anywhere; }
          .markdown > :first-child { margin-top: 0; }
          .markdown > :last-child { margin-bottom: 0; }
          .markdown p { margin: 0 0 10px; }
          .markdown h1, .markdown h2, .markdown h3, .markdown h4, .markdown h5, .markdown h6 { margin: 16px 0 8px; line-height: 1.3; font-weight: 600; }
          .markdown h1 { font-size: 18px; }
          .markdown h2 { font-size: 16px; }
          .markdown h3 { font-size: 14px; }
          .markdown h4, .markdown h5, .markdown h6 { font-size: 13px; color: var(--muted); }
          .markdown ul, .markdown ol { margin: 0 0 10px; padding-left: 22px; }
          .markdown li { margin: 2px 0; }
          .markdown li > ul, .markdown li > ol { margin: 2px 0; }
          .markdown a { color: var(--fg); text-decoration: underline; text-underline-offset: 2px; }
          .markdown code { font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 0.92em; background: var(--bg); border: 1px solid var(--border); border-radius: 2px; padding: 1px 4px; }
          .markdown pre { margin: 0 0 10px; padding: 10px 12px; background: var(--bg); border: 1px solid var(--border); overflow-x: auto; }
          .markdown pre code { background: none; border: none; padding: 0; font-size: 12px; line-height: 1.45; }
          .markdown blockquote { margin: 0 0 10px; padding: 2px 12px; border-left: 2px solid var(--border); color: var(--muted); }
          .markdown hr { border: none; border-top: 1px solid var(--border); margin: 16px 0; }
          .markdown table { border-collapse: collapse; margin: 0 0 10px; }
          .markdown th, .markdown td { border: 1px solid var(--border); padding: 6px 10px; text-align: left; }
          .markdown th { color: var(--muted); font-weight: 500; }
          .markdown img { max-width: 100%; }
          .codex-msg .markdown { font-size: 13px; }
          .codex-msg.reasoning .markdown { color: var(--muted); font-style: italic; }
          .codex-msg[data-role='developer'] .markdown { color: var(--muted); }
          .stats-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; align-items: start; }
          .stats-card { min-width: 0; }
          .bar-chart { display: grid; gap: 10px; }
          .bar-row { display: grid; grid-template-columns: minmax(150px, 220px) minmax(120px, 1fr) 48px; gap: 12px; align-items: center; min-height: 36px; }
          .bar-person { display: flex; align-items: center; gap: 10px; min-width: 0; }
          .bar-person img { width: 28px; height: 28px; border: 1px solid var(--border); background: var(--bg); flex: 0 0 auto; }
          .bar-person span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
          .bar-track { height: 12px; background: var(--bg); border: 1px solid var(--border); overflow: hidden; }
          .bar-fill { height: 100%; min-width: 3px; background: var(--fg); }
          .bar-count { text-align: right; font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; font-size: 12px; color: var(--fg); font-variant-numeric: tabular-nums; }
          /* Inline IR graph - server-rendered SVG, no JS library */
          /* max-width is set inline on the SVG element to the natural content width
             so a single-node workflow does not stretch to fill the card. */
          .graph { display: block; width: 100%; }
          .gnode rect { fill: #101012; stroke: var(--border); }
          .gnode text { fill: var(--fg); font-family: ui-monospace, "SF Mono", "JetBrains Mono", monospace; }
          .gnode .gnode-label { font-size: 12px; font-weight: 600; }
          .gnode .gnode-id { font-size: 10px; }
          .gnode .gnode-detail { font-size: 10px; }
          .gnode.succeeded rect { stroke: rgba(106,210,138,0.5); }
          .gnode.running rect { stroke: rgba(216,164,90,0.6); }
          .gnode.pending rect { stroke: var(--border); }
          .gnode.failed rect { stroke: rgba(255,107,107,0.5); }
          .gnode.skipped rect { stroke: rgba(216,164,90,0.35); }
          .gnode.gate rect { stroke-dasharray: 4 3; }
          .gnode.gtrigger rect { fill: #0a0a12; stroke: rgba(106,138,210,0.55); stroke-dasharray: 4 3; }
          .gnode.gtrigger .gnode-label { fill: rgba(106,138,210,0.9); }
          .gedge { stroke: var(--border); fill: none; }
          .garrow { fill: var(--muted); }
          @media (max-width: 800px) {
            main.shell { padding: 20px 16px; }
            header.bar { align-items: flex-start; }
            nav.tabs { flex-wrap: wrap; gap: 10px 14px; }
            .stats-grid { grid-template-columns: 1fr; }
            .bar-row { grid-template-columns: minmax(0, 1fr) 72px 36px; }
          }
        </style>
        <script defer src="/vendor/phoenix/phoenix.js"></script>
        <script defer src="/vendor/phoenix_html/phoenix_html.js"></script>
        <script defer src="/vendor/phoenix_live_view/phoenix_live_view.js"></script>
        <script>
          window.addEventListener("DOMContentLoaded", function () {
            var csrfToken = document.querySelector("meta[name='csrf-token']")?.getAttribute("content");
            if (!window.Phoenix || !window.LiveView) return;
            var liveSocket = new window.LiveView.LiveSocket("/live", window.Phoenix.Socket, { params: { _csrf_token: csrfToken } });
            liveSocket.connect();
            window.liveSocket = liveSocket;
          });
        </script>
      </head>
      <body>
        {@inner_content}
      </body>
    </html>
    """
  end

  @spec app(map()) :: Rendered.t()
  def app(assigns) do
    # Every call site passes active_tab explicitly; default to :ir if
    # someone forgets, using Map.put_new (not assign_new, which expects a
    # socket / change-tracked assigns map and crashes on a plain one - was
    # the cause of every LiveView route returning 500 after PR #21).
    assigns = Map.put_new(assigns, :active_tab, :ir)

    ~H"""
    <main class="shell">
      <header class="bar">
        <div>
          <h1>
            <span class="logo-mark" aria-hidden="true">🎷</span>
            <span><span class="logo-bracket">[</span><span class="logo-name">sym</span><span class="logo-bracket">]</span>phony</span>
          </h1>
          <nav class="tabs">
            <a href="/" class={if @active_tab == :ir, do: "active", else: ""}>runs</a>
            <a href="/workflows" class={if @active_tab == :workflows, do: "active", else: ""}>workflows</a>
            <a href="/skills" class={if @active_tab == :skills, do: "active", else: ""}>skills</a>
            <a href="/statistics" class={if @active_tab == :statistics, do: "active", else: ""}>statistics</a>
          </nav>
        </div>
      </header>
      {@inner_content}
    </main>
    """
  end
end
