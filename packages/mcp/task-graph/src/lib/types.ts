// Mirrors the `tasks` Python helper's data model. The SQLite file produced by
// `tasks.seed(...)` is the single source of truth; this is just the typed view of
// it on the client.

export type Category =
  | 'Design'
  | 'Backend'
  | 'Frontend'
  | 'Infra'
  | 'Data'
  | 'QA'
  | 'Docs';

export type Status = 'done' | 'in-progress' | 'ready' | 'blocked';

export interface Task {
  id: string;
  title: string;
  category: Category;
  estimate: number;
  complete: boolean;
  active: boolean;
  deps: string[];
}

export function statusOf(task: Task, byId: Map<string, Task>): Status {
  if (task.complete) return 'done';
  const ready = task.deps.every((id) => byId.get(id)?.complete);
  if (!ready) return 'blocked';
  return task.active ? 'in-progress' : 'ready';
}

export const STATUS_META: Record<Status, { label: string; color: string }> = {
  done: { label: 'Done', color: '#3fb950' },
  'in-progress': { label: 'In progress', color: '#58a6ff' },
  ready: { label: 'Ready', color: '#d29922' },
  blocked: { label: 'Blocked', color: '#f85149' },
};

export const CATEGORY_COLORS: Record<Category, string> = {
  Design: '#bc8cff',
  Backend: '#58a6ff',
  Frontend: '#3fb950',
  Infra: '#f0883e',
  Data: '#39c5cf',
  QA: '#f85149',
  Docs: '#d2a8ff',
};
