"""A tiny read-only web dashboard over the execution store.

Auto-started by the CLI. It renders every execution (running first) with its live
output, polling the SQLite store the kernel writes to, so a human can watch all
the running "things" and their output like a notebook view. Deliberately simple:
one HTML page that fetches ``/api/jobs`` once a second. Each execution renders
its rich outputs like a notebook: a polars DataFrame as its HTML table, a
matplotlib figure as an image, falling back to text for everything else.
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
 details.code{margin:6px 0 0}
 details.code>summary{cursor:pointer;color:#565f89;font-size:11px;list-style:none;user-select:none;display:inline-block}
 details.code>summary::-webkit-details-marker{display:none}
 details.code>summary::before{content:"▸ code"}
 details.code[open]>summary::before{content:"▾ code"}
 details.code>pre{color:#565f89;max-height:320px}
 .res{color:#9ece6a}
 .empty{color:#565f89}
 .rich{background:#fff;color:#111;padding:8px;border-radius:4px;margin:6px 0 0;overflow:auto;max-height:460px}
 .rich table{border-collapse:collapse;font:12px/1.4 ui-monospace,Menlo,monospace}
 .rich th,.rich td{border:1px solid #d0d7de;padding:2px 7px;text-align:right}
 .rich th{background:#f6f8fa}
 .img{display:block;max-width:100%;margin:6px 0 0;border-radius:4px;background:#fff}
 .layout{display:flex;gap:16px;align-items:flex-start}
 #main{flex:1 1 auto;min-width:0}
 .sidebar{flex:0 0 540px;position:sticky;top:0;max-height:100vh;overflow:auto}
 .res-card{border:1px solid #2a2e42;border-radius:6px;margin:0 0 10px;padding:8px}
 .res-card.live{border-color:#7dcfff}
 .res-card.error{border-color:#f7768e}
 .res-hdr{display:flex;gap:8px;align-items:center;margin:0 0 6px}
 .res-dot{width:8px;height:8px;border-radius:50%;background:#7dcfff;flex:none}
 .res-card.error .res-dot{background:#f7768e}
 .res-title{color:#bb9af7;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
 .res-kind{color:#565f89;font-size:11px;margin-left:auto;flex:none}
 .res-body{overflow:auto;max-height:60vh}
</style></head><body>
<div class="layout">
 <div id="main"><h1>ix-mcp executions</h1><div id="jobs"></div></div>
 <aside class="sidebar"><h1>resources</h1><div id="resources"></div></aside>
</div>
<script>
async function tick(){
 try{
  const r=await fetch('api/jobs');const js=await r.json();
  const el=document.getElementById('jobs');
  if(!js.length){el.innerHTML='<div class="empty">no executions yet</div>';return;}
  js.sort((a,b)=>a.started_at-b.started_at);          // oldest at top, newest at bottom
  const nearBottom=(window.innerHeight+window.scrollY)>=(document.body.scrollHeight-120);
  el.innerHTML=js.map(j=>{
   const dur=((j.ended_at||Date.now()/1000)-j.started_at).toFixed(1);
   const esc=s=>(s||'').replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));
   // Rich outputs render like a notebook cell. The dashboard is read-only over the
   // tailnet (the trust boundary); the HTML is the agent's own code output, so it
   // is injected as-is rather than re-sanitized.
   const rich=o=>{const d=(o&&o.data)||{};
     if(d['image/png'])return `<img class="img" src="data:image/png;base64,${d['image/png']}">`;
     if(d['image/jpeg'])return `<img class="img" src="data:image/jpeg;base64,${d['image/jpeg']}">`;
     if(d['image/svg+xml'])return `<div class="rich">${d['image/svg+xml']}</div>`;
     if(d['text/html'])return `<div class="rich">${d['text/html']}</div>`;
     if(d['text/markdown'])return `<pre>${esc(d['text/markdown'])}</pre>`;
     if(d['text/plain'])return `<pre class="res">${esc(d['text/plain'])}</pre>`;
     return '';};
   const richOut=(j.outputs&&j.outputs.length)
     ? j.outputs.map(rich).join('')
     : (j.result?`<pre class="res">${esc(j.result)}</pre>`:'');
   return `<div class="job ${j.status}">
     <div class="hdr"><span class="st">${j.status}</span>
     <span class="id">${j.id}</span><span class="name">${esc(j.name)}</span>
     <span class="dur">${dur}s</span></div>
     <details class="code"><summary></summary><pre>${esc(j.code)}</pre></details>
     ${j.output?`<pre>${esc(j.output)}</pre>`:''}
     ${richOut}
     ${j.error&&!j.output.includes(j.error)?`<pre class="error">${esc(j.error)}</pre>`:''}
   </div>`;
  }).join('');
  if(nearBottom) window.scrollTo(0, document.body.scrollHeight);
 }catch(e){}
}
async function tickResources(){
 try{
  const r=await fetch('api/resources');const rs=await r.json();
  const el=document.getElementById('resources');
  if(!rs.length){el.innerHTML='<div class="empty">no live resources</div>';return;}
  const esc=s=>(s||'').replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));
  // A resource's html is the agent's own live render (a terminal screen, a custom
  // widget). The dashboard is read-only over the tailnet (the trust boundary), so
  // it is injected as-is, exactly like job output above.
  el.innerHTML=rs.map(x=>`<div class="res-card ${esc(x.status)}">
    <div class="res-hdr"><span class="res-dot"></span>
    <span class="res-title">${esc(x.title)}</span>
    <span class="res-kind">${esc(x.kind)}</span></div>
    <div class="res-body">${x.html||''}</div>
  </div>`).join('');
 }catch(e){}
}
tick();setInterval(tick,1000);
tickResources();setInterval(tickResources,1000);
</script></body></html>"""


async def start(config: Config) -> web.AppRunner:
    app = web.Application()
    conn = store.connect(config.store_path)

    async def index(_request: web.Request) -> web.Response:
        return web.Response(text=_PAGE, content_type="text/html")

    async def jobs(_request: web.Request) -> web.Response:
        return web.json_response(store.recent(conn, limit=200))

    async def resources(_request: web.Request) -> web.Response:
        return web.json_response(store.live_resources(conn))

    app.router.add_get("/", index)
    app.router.add_get("/api/jobs", jobs)
    app.router.add_get("/api/resources", resources)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, config.host, config.dashboard_port)
    await site.start()
    return runner
