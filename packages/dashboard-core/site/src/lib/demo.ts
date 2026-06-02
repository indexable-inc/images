// Front-end-only demo data so the dashboard can be explored without a running hub:
// load the page with `?demo`. Mirrors the kinds the Rust `dashboard demo` produces
// and adds a traced exec pane to showcase the inline-trace view. Statically
// imported (it is tiny) but only invoked from App.svelte when `?demo` is present.
import { store } from './stream.svelte';

export function seedDemo(): void {
  const now = Date.now();
  const SEP = String.fromCharCode(0x1f);

  store.panes = {
    // Inline-trace: a traced exec — each printed line's output shown inline beside it.
    [`demo${SEP}files`]: {
      kind: 'exec',
      title: 'list files',
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
      created_at: now - 3000,
    },
    // Auto-detected JSON output, syntax-highlighted.
    [`demo${SEP}json`]: {
      kind: 'exec',
      title: 'status',
      subtitle: 'eval · demo',
      lang: 'python',
      source: 'import json\njson.dumps({"service": "dashboard", "ok": True, "panes": 13})',
      result: '{"service": "dashboard", "ok": true, "panes": 13, "nested": {"a": [1, 2, 3]}}',
      running: false,
      ok: true,
      created_at: now - 9000,
    },
    // An error: prints before failing, traceback below the trace; red LED.
    [`demo${SEP}boom`]: {
      kind: 'exec',
      title: 'oops',
      subtitle: 'exec · demo',
      lang: 'python',
      source: 'x = [1, 2, 3]\nprint("before the error")\nprint(x[10])',
      stdout: 'before the error\n',
      stderr:
        'Traceback (most recent call last):\n  File "<ix-mcp exec>", line 3, in <module>\nIndexError: list index out of range\n',
      trace: JSON.stringify([{ line: 2, text: 'before the error\n' }]),
      running: false,
      ok: false,
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
    'demo-data': {
      kind: 'data',
      title: 'data pane',
      renderer: 'kv',
      body: JSON.stringify({ tick: 42, status: 'even', load: 0.42, nested: { a: 1, b: [1, 2, 3] } }),
      created_at: now - 30000,
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
  store.status = '6 panes · demo';
}
