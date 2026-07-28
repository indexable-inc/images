// Front-end-only demo data so the dashboard can be explored without a running hub:
// load the page with `?demo`. Mirrors the kinds the Rust `dashboard demo` produces
// and adds a traced exec pane to showcase the inline-trace view. Statically
// imported (it is tiny) but only invoked from App.svelte when `?demo` is present.
import { store } from './stream.svelte';

export function seedDemo(): void {
  const now = Date.now();
  const SEP = String.fromCharCode(0x1f);

  store.panes = {
    // Each producer (MCP client) publishes a reserved `__session__` pane naming
    // the session; the feed reads it to label the session selector and never
    // shows it as a run. Two scopes here exercise the picker.
    [`demo${SEP}__session__`]: {
      kind: 'data',
      renderer: 'session',
      title: 'refactor the auth flow',
      body: JSON.stringify({ name: 'refactor the auth flow', client: 'claude-code 2.1.0' }),
      created_at: now - 32000,
    },
    [`agent2${SEP}__session__`]: {
      kind: 'data',
      renderer: 'session',
      title: 'claude-code · nix-web-monitor',
      body: JSON.stringify({ name: '', client: 'claude-code 2.1.0' }),
      created_at: now - 12000,
    },
    // A run belonging to the second session, so selecting it shows a distinct set.
    [`agent2${SEP}probe`]: {
      kind: 'exec',
      title: 'probe service health',
      subtitle: 'exec · demo',
      lang: 'python',
      source: 'import httpx\nr = await httpx.AsyncClient().get("http://localhost:8080/health")\nprint(r.status_code, r.text)',
      stdout: '200 ok\n',
      running: false,
      ok: true,
      duration_ms: 92,
      created_at: now - 11000,
    },
    // Inline-trace: a traced exec — each printed line's output shown inline beside it.
    [`demo${SEP}files`]: {
      kind: 'exec',
      title: 'listing tracked files',
      subtitle: 'exec · demo',
      lang: 'python',
      source:
        'import os, subprocess\n' +
        'repo = os.path.expanduser("~/Projects/indexable-inc/index")\n' +
        'files = subprocess.run(["git", "-C", repo, "ls-files"], capture_output=True, text=True).stdout.splitlines()\n' +
        'for f in files[:3]:\n' +
        '    print(f)\n' +
        'print("total:", len(files))',
      stdout: 'a.py\nb.py\nc.py\ntotal: 1287\n',
      trace: JSON.stringify([
        { line: 5, text: 'a.py\nb.py\nc.py\n' },
        { line: 6, text: 'total: 1287\n' },
      ]),
      running: false,
      ok: true,
      duration_ms: 1240,
      created_at: now - 3000,
    },
    // Auto-detected JSON output, syntax-highlighted.
    [`demo${SEP}json`]: {
      kind: 'exec',
      title: 'checking dashboard status',
      subtitle: 'eval · demo',
      lang: 'python',
      source: 'import json\njson.dumps({"service": "dashboard", "ok": True, "panes": 13})',
      result: '{"service": "dashboard", "ok": true, "panes": 13, "nested": {"a": [1, 2, 3]}}',
      running: false,
      ok: true,
      duration_ms: 38,
      created_at: now - 9000,
    },
    // A run's rich output (a table/plot/image) arrives as a `<id>/out` html pane
    // beside its exec. The feed folds it into the run's detail rather than showing
    // it as a duplicate row.
    [`demo${SEP}json/out`]: {
      kind: 'html',
      title: 'checking dashboard status',
      subtitle: 'output',
      body:
        '<table style="border-collapse:collapse;font:12px ui-monospace,monospace">' +
        '<tr><th style="text-align:left;padding:3px 12px 3px 0">service</th><th style="text-align:left;padding:3px 0">ok</th></tr>' +
        '<tr><td style="padding:3px 12px 3px 0">dashboard</td><td style="padding:3px 0;color:#52c98e">true</td></tr></table>',
      created_at: now - 9000,
    },
    // An error: prints before failing, traceback below the trace; red LED.
    [`demo${SEP}boom`]: {
      kind: 'exec',
      title: 'indexing into a list',
      subtitle: 'exec · demo',
      lang: 'python',
      source: 'x = [1, 2, 3]\nprint("before the error")\nprint(x[10])',
      stdout: 'before the error\n',
      stderr:
        'Traceback (most recent call last):\n  File "<ix-mcp exec>", line 3, in <module>\nIndexError: list index out of range\n',
      trace: JSON.stringify([{ line: 2, text: 'before the error\n' }]),
      running: false,
      ok: false,
      duration_ms: 5,
      created_at: now - 20000,
    },
    'demo-term': {
      kind: 'terminal',
      title: 'demo',
      subtitle: '--tick',
      rows: 3,
      cols: 40,
      alive: true,
      body: '\x1b[32mtick 7\x1b[0m\n#######\nany resource is a pane',
      cursor_visible: false,
      created_at: now - 30000,
    },
    // The board only tiles live *resources* — TUIs and browsers. A second TUI and
    // a browser resource (an `html` pane keyed `resource/<id>`, subtitle = its
    // kind) exercise the tiling layout.
    'demo-term2': {
      kind: 'terminal',
      title: 'btop',
      subtitle: 'tui',
      rows: 6,
      cols: 44,
      alive: true,
      body:
        'cpu \x1b[32m▁▂▃▅▇▆▄▂\x1b[0m  41%\n' +
        'mem \x1b[36m████████░░░░\x1b[0m 63%\n' +
        'net \x1b[33m↑ 1.2M  ↓ 8.4M\x1b[0m\n' +
        '\x1b[90mpid   cmd          cpu\x1b[0m\n' +
        '1042  python        22%\n' +
        '  77  node           9%',
      cursor_visible: false,
      created_at: now - 26000,
    },
    [`demo${SEP}resource/browser`]: {
      kind: 'html',
      title: 'localhost:5173',
      subtitle: 'browser',
      body:
        '<div style="font:13px ui-sans-serif,system-ui;height:100%;display:flex;flex-direction:column;background:#fff;color:#111">' +
        '<div style="display:flex;gap:6px;align-items:center;padding:8px 10px;border-bottom:1px solid #e5e5e5;background:#f6f6f7">' +
        '<span style="color:#bbb">←  →  ⟳</span>' +
        '<span style="flex:1;background:#fff;border:1px solid #e0e0e0;border-radius:6px;padding:3px 9px;color:#555">localhost:5173</span></div>' +
        '<div style="flex:1;display:grid;place-content:center;text-align:center;gap:6px">' +
        '<div style="font-size:34px">🌐</div><div style="font-weight:600">ix dashboard</div>' +
        '<div style="color:#888">a live browser resource</div></div></div>',
      created_at: now - 18000,
    },
    'demo-data': {
      kind: 'data',
      title: 'data pane',
      renderer: 'kv',
      body: JSON.stringify({ tick: 42, status: 'even', load: 0.42, nested: { a: 1, b: [1, 2, 3] } }),
      created_at: now - 30000,
    },
    // A `data` pane with the `namespace` renderer: a Python session's live globals,
    // exercising the named-renderer dispatch (renderer → NamespaceBody, else tree).
    [`demo${SEP}ns`]: {
      kind: 'data',
      title: 'Namespace',
      subtitle: 'default',
      renderer: 'namespace',
      body: JSON.stringify([
        { name: 'X_train', type: 'ndarray', kind: 'array', repr: '', size: 156_800_000, shape: '50000×784' },
        { name: 'df', type: 'DataFrame', kind: 'frame', repr: '', size: 84_000_000, shape: '1204853×8' },
        { name: 'model', type: 'Sequential', kind: 'object', repr: '<Sequential: 4 layers>', size: 4_800_000, shape: '' },
        // A mapping with nested children, to exercise the recursive tree: `config`
        // holds a nested `paths` dict (expandable two levels deep).
        {
          name: 'config',
          type: 'dict',
          kind: 'mapping',
          repr: "{'lr': 0.001, 'paths': {...}, ...}",
          size: 1_240,
          shape: 'len 4',
          children: [
            { name: "'lr'", type: 'float', kind: 'scalar', repr: '0.001', size: 24, shape: '' },
            { name: "'epochs'", type: 'int', kind: 'scalar', repr: '10', size: 28, shape: '' },
            {
              name: "'paths'",
              type: 'dict',
              kind: 'mapping',
              repr: "{'data': '/mnt/data', 'ckpt': '/mnt/ckpt'}",
              size: 320,
              shape: 'len 2',
              children: [
                { name: "'data'", type: 'str', kind: 'text', repr: "'/mnt/data'", size: 58, shape: '' },
                { name: "'ckpt'", type: 'str', kind: 'text', repr: "'/mnt/ckpt'", size: 58, shape: '' },
              ],
            },
            { name: "'tags'", type: 'list', kind: 'sequence', repr: "['a', 'b']", size: 120, shape: 'len 2',
              children: [
                { name: '[0]', type: 'str', kind: 'text', repr: "'a'", size: 50, shape: '' },
                { name: '[1]', type: 'str', kind: 'text', repr: "'b'", size: 50, shape: '' },
              ] },
          ],
        },
        {
          name: 'results',
          type: 'list',
          kind: 'sequence',
          repr: '[{...}, {...}, ...]',
          size: 31_700,
          shape: 'len 20',
          children: [
            { name: '[0]', type: 'dict', kind: 'mapping', repr: "{'id': 0, 'score': 0.91}", size: 232, shape: 'len 2' },
            { name: '[1]', type: 'dict', kind: 'mapping', repr: "{'id': 1, 'score': 0.88}", size: 232, shape: 'len 2' },
            { name: '…', type: '', kind: 'object', repr: '+18 more', size: 0, shape: '' },
          ],
        },
        { name: 'result', type: 'int', kind: 'scalar', repr: '4', size: 28, shape: '' },
        { name: 'pl', type: 'module', kind: 'module', repr: 'polars 1.12.0', size: 0, shape: '' },
        { name: 'embed', type: 'function', kind: 'function', repr: '<function embed>', size: 0, shape: '' },
      ]),
      created_at: now - 15000,
    },
    'demo-html': {
      kind: 'html',
      title: 'html pane',
      body:
        '<div style="font:14px ui-monospace,monospace;padding:14px;color:#89b4fa">' +
        '<div style="font-size:28px">42</div>' +
        '<div style="opacity:.6">a producer-rendered HTML view</div></div>',
      created_at: now - 30000,
    },
  };
  store.live = true;
  store.status = 'demo';
}
