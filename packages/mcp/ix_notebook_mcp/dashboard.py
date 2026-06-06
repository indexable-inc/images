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
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>ix · executions</title>
<style>
 :root{
  --bg:#0b0b0c; --panel:#141416; --panel-2:#1a1a1d; --inset:#101012;
  --line:#242427; --line-2:#2e2e33;
  --text:#e6e6e6; --dim:#9a9aa0; --muted:#6a6a70; --faint:#45454b;
  --active:#f2f2f2; --err:#cf5a5a;
  --mono:ui-monospace,"SF Mono",SFMono-Regular,Menlo,"Cascadia Code",monospace;
 }
 *{box-sizing:border-box}
 html{scrollbar-color:var(--line-2) transparent}
 body{background:var(--bg);color:var(--text);font:13px/1.55 var(--mono);margin:0;
   -webkit-font-smoothing:antialiased;text-rendering:optimizeLegibility}
 ::selection{background:#33333a;color:#fff}

 header.top{position:sticky;top:0;z-index:5;display:flex;align-items:center;gap:12px;
   padding:11px 18px;background:rgba(11,11,12,.86);backdrop-filter:blur(8px);
   border-bottom:1px solid var(--line)}
 .brand{font-size:11px;letter-spacing:.22em;text-transform:uppercase;color:var(--dim);font-weight:600}
 .brand b{color:var(--text);font-weight:600}
 .spacer{flex:1}
 .stat{font-size:11px;letter-spacing:.04em;color:var(--muted)}
 .stat b{color:var(--text);font-weight:600}
 .dot{display:inline-block;width:6px;height:6px;background:var(--active);margin-right:6px;vertical-align:middle}

 .wrap{display:flex;gap:18px;align-items:flex-start;padding:18px;max-width:1600px;margin:0 auto}
 #main{flex:1 1 auto;min-width:0}
 .sidebar{flex:0 0 520px;position:sticky;top:62px;max-height:calc(100vh - 78px);overflow:auto}
 @media(max-width:1100px){.wrap{flex-direction:column}.sidebar{flex:none;width:100%;position:static;max-height:none}}

 .sec{font-size:10px;letter-spacing:.2em;text-transform:uppercase;color:var(--muted);
   margin:0 0 12px;padding-bottom:7px;border-bottom:1px solid var(--line);font-weight:600}

 /* execution card */
 .job{background:var(--panel);border:1px solid var(--line);border-left:2px solid var(--line-2);
   margin:0 0 9px;padding:11px 14px}
 .job.running{border-left-color:var(--active)}
 .job.error{border-left-color:var(--err)}
 .hdr{display:flex;gap:9px;align-items:center;flex-wrap:wrap}
 .st{font-size:9px;letter-spacing:.12em;text-transform:uppercase;font-weight:600;
   padding:2px 6px;border:1px solid var(--line-2);color:var(--dim)}
 .running .st{background:var(--active);color:#0b0b0c;border-color:var(--active)}
 .error .st{color:var(--err);border-color:#43282b}
 .cancelled .st{color:var(--muted)}
 .id{color:var(--muted);font-size:12px}
 .name{color:var(--text);font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
 .dur{margin-left:auto;color:var(--faint);font-size:11px;font-variant-numeric:tabular-nums;flex:none}

 details.code{margin:8px 0 0}
 details.code>summary{cursor:pointer;color:var(--faint);font-size:10px;letter-spacing:.14em;
   text-transform:uppercase;list-style:none;user-select:none;display:inline-block}
 details.code>summary::-webkit-details-marker{display:none}
 details.code>summary::before{content:"+ source"}
 details.code[open]>summary::before{content:"− source"}
 details.code>pre{color:var(--dim);background:var(--inset);border:1px solid var(--line);
   padding:9px 11px;max-height:340px}

 pre{white-space:pre-wrap;word-break:break-word;margin:8px 0 0;color:var(--dim);
   max-height:340px;overflow:auto;font-size:12px}
 pre.res{color:var(--text)}
 pre.error{color:var(--err)}
 .empty{color:var(--faint);font-style:italic;font-size:12px;padding:2px 0}

 /* rich notebook output: blends into the surface, no white card */
 .rich{background:var(--inset);border:1px solid var(--line);padding:10px;margin:8px 0 0;
   overflow:auto;max-height:480px;color:var(--text)}
 .rich table{border-collapse:collapse;font:12px/1.45 var(--mono);color:var(--text)}
 .rich th,.rich td{border-bottom:1px solid var(--line);padding:3px 12px;text-align:right;white-space:nowrap}
 .rich th{color:var(--dim);font-weight:600;border-bottom:1px solid var(--line-2)}
 .rich tr:hover td{background:var(--panel-2)}
 .img{display:block;max-width:100%;margin:8px 0 0;border:1px solid var(--line);background:#fff}

 /* resources */
 .res-card{background:var(--panel);border:1px solid var(--line);border-left:2px solid var(--line-2);
   margin:0 0 9px;padding:9px 11px}
 .res-card.live{border-left-color:var(--active)}
 .res-card.error{border-left-color:var(--err)}
 .res-hdr{display:flex;gap:8px;align-items:center;margin:0 0 7px}
 .res-dot{width:6px;height:6px;background:var(--active);flex:none}
 .res-card.error .res-dot{background:var(--err)}
 .res-title{color:var(--text);font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
 .res-kind{margin-left:auto;color:var(--faint);font-size:10px;letter-spacing:.1em;
   text-transform:uppercase;flex:none}
 .res-body{overflow:auto;max-height:62vh}
 ::-webkit-scrollbar{width:9px;height:9px}
 ::-webkit-scrollbar-thumb{background:var(--line-2)}
 ::-webkit-scrollbar-track{background:transparent}
</style></head><body>
<header class="top">
  <span class="brand"><b>ix</b> &middot; mcp</span>
  <span class="spacer"></span>
  <span class="stat" id="run-stat"></span>
</header>
<div class="wrap">
 <div id="main"><div class="sec">executions</div><div id="jobs"></div></div>
 <aside class="sidebar"><div class="sec">resources</div><div id="resources"></div></aside>
</div>
<script>
const esc=s=>(s||'').replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));
async function tick(){
 try{
  const r=await fetch('api/jobs');const js=await r.json();
  const el=document.getElementById('jobs');
  const running=js.filter(j=>j.status==='running').length;
  document.getElementById('run-stat').innerHTML=
    (running?`<span class="dot"></span><b>${running}</b> running &nbsp;`:'')+`<b>${js.length}</b> total`;
  if(!js.length){el.innerHTML='<div class="empty">no executions yet</div>';return;}
  js.sort((a,b)=>a.started_at-b.started_at);          // oldest at top, newest at bottom
  const nearBottom=(window.innerHeight+window.scrollY)>=(document.body.scrollHeight-120);
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
  el.innerHTML=js.map(j=>{
   const dur=((j.ended_at||Date.now()/1000)-j.started_at).toFixed(1);
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
     ${j.error&&!(j.output||'').includes(j.error)?`<pre class="error">${esc(j.error)}</pre>`:''}
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
