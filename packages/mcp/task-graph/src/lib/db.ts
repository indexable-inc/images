// Read the task graph from a real SQLite file in the browser, via sql.js (a
// WASM build of SQLite). The file is produced by the `tasks` Python helper
// (`tasks.seed("tasks.sqlite")`) and shipped as a static asset.

import initSqlJs, { type SqlJsStatic } from 'sql.js';
import wasmUrl from 'sql.js/dist/sql-wasm.wasm?url';
import type { Category, Task } from './types';

let sqlPromise: Promise<SqlJsStatic> | null = null;
function sql(): Promise<SqlJsStatic> {
  sqlPromise ??= initSqlJs({ locateFile: () => wasmUrl });
  return sqlPromise;
}

/** Load every task (with its dependencies) from the SQLite file at `url`. */
export async function loadTasks(url = 'tasks.sqlite'): Promise<Task[]> {
  const [SQL, buf] = await Promise.all([
    sql(),
    fetch(url).then((r) => {
      if (!r.ok) throw new Error(`Failed to fetch ${url}: ${r.status}`);
      return r.arrayBuffer();
    }),
  ]);

  const db = new SQL.Database(new Uint8Array(buf));
  try {
    const deps = new Map<string, string[]>();
    const depRes = db.exec('SELECT task_id, depends_on FROM deps');
    if (depRes.length) {
      for (const [taskId, dependsOn] of depRes[0].values) {
        const list = deps.get(taskId as string) ?? [];
        list.push(dependsOn as string);
        deps.set(taskId as string, list);
      }
    }

    const res = db.exec(
      'SELECT id, title, category, estimate, complete, active FROM tasks ORDER BY id',
    );
    if (!res.length) return [];

    return res[0].values.map((row): Task => {
      const [id, title, category, estimate, complete, active] = row;
      return {
        id: id as string,
        title: title as string,
        category: category as Category,
        estimate: estimate as number,
        complete: !!(complete as number),
        active: !!(active as number),
        deps: deps.get(id as string) ?? [],
      };
    });
  } finally {
    db.close();
  }
}
