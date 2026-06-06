"""A tiny read-only web dashboard over the execution store.

Auto-started by the CLI. It renders every execution (running first) with its live
output, polling the SQLite store the kernel writes to, so a human can watch all
the running "things" and their output like a notebook view. Deliberately simple:
one HTML page that fetches ``/api/jobs`` once a second. Rich output (images,
HTML tables) is a follow-up; v1 shows code, status, duration, output, and result.
"""

from __future__ import annotations

from aiohttp import web

from . import store
from .config import Config

_PAGE = """<!doctype html>
<html><head><meta charset="utf-8"><title>ix-mcp</title>
<style>
 body{background:#1a1b26;color:#c0caf5;font:13px/1.5 ui-monospace,Menlo,monospace;margin:0;padding:16px}
 h1{font-size:14px;color:#7aa2f7;margin:0 0 12px}
 .job{border:1px solid #2a2e42;border-radius:6px;margin:0 0 10px;padding:10px}
 .job.running{border-color:#9ece6a}
 .hdr{display:flex;gap:10px;align-items:baseline;flex-wrap:wrap}
 .id{color:#7dcfff}.name{color:#bb9af7}.dur{color:#565f89;margin-left:auto}
 .st{padding:0 6px;border-radius:4px;font-size:11px}
 .running .st{background:#9ece6a;color:#1a1b26}.done .st{background:#2a2e42}
 .error .st{background:#f7768e;color:#1a1b26}.cancelled .st{background:#565f89;color:#1a1b26}
 pre{white-space:pre-wrap;word-break:break-word;margin:6px 0 0;color:#a9b1d6;max-height:320px;overflow:auto}
 .code{color:#565f89;max-height:80px}.res{color:#9ece6a}
 .empty{color:#565f89}
</style></head><body>
<h1>ix-mcp executions</h1><div id="jobs"></div>
<script>
async function tick(){
 try{
  const r=await fetch('api/jobs');const js=await r.json();
  const el=document.getElementById('jobs');
  if(!js.length){el.innerHTML='<div class="empty">no executions yet</div>';return;}
  el.innerHTML=js.map(j=>{
   const dur=((j.ended_at||Date.now()/1000)-j.started_at).toFixed(1);
   const esc=s=>(s||'').replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));
   return `<div class="job ${j.status}">
     <div class="hdr"><span class="st">${j.status}</span>
     <span class="id">${j.id}</span><span class="name">${esc(j.name)}</span>
     <span class="dur">${dur}s</span></div>
     <pre class="code">${esc(j.code)}</pre>
     ${j.output?`<pre>${esc(j.output)}</pre>`:''}
     ${j.result?`<pre class="res">${esc(j.result)}</pre>`:''}
     ${j.error&&!j.output.includes(j.error)?`<pre class="error">${esc(j.error)}</pre>`:''}
   </div>`;
  }).join('');
 }catch(e){}
}
tick();setInterval(tick,1000);
</script></body></html>"""


async def start(config: Config) -> web.AppRunner:
    app = web.Application()
    conn = store.connect(config.store_path)

    async def index(_request: web.Request) -> web.Response:
        return web.Response(text=_PAGE, content_type="text/html")

    async def jobs(_request: web.Request) -> web.Response:
        return web.json_response(store.recent(conn, limit=200))

    app.router.add_get("/", index)
    app.router.add_get("/api/jobs", jobs)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, config.host, config.dashboard_port)
    await site.start()
    return runner
