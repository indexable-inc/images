// Extract GitHub PR / issue references from a thread's messages.
//
// Three forms are recognized:
//   1. Full URLs like https://github.com/owner/repo/pull/123
//   2. Cross-repo refs like owner/repo#123
//   3. Bare #123 — qualified using the thread's `repo` field when set
//
// References dedupe by (owner, repo, number). The `kind` is best-effort
// from the URL form; cross-repo and bare refs default to 'unknown' and
// the link uses /issues/ (GitHub will redirect PRs automatically).

import type { Message } from './types';

export type IssueKind = 'pull' | 'issues' | 'unknown';

export interface IssueRef {
  owner: string;
  repo: string;
  number: number;
  kind: IssueKind;
  url: string;
  label: string;
}

const URL_RE = /https?:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/(pull|issues)\/(\d+)/g;
const CROSS_RE = /(?<![\w/])([\w.-]+)\/([\w.-]+)#(\d+)/g;
const BARE_RE = /(?<![\w/#])#(\d+)\b/g;

function pushUnique(out: Map<string, IssueRef>, ref: IssueRef) {
  const key = `${ref.owner}/${ref.repo}#${ref.number}`;
  if (!out.has(key)) out.set(key, ref);
}

export function extractIssues(messages: Message[], defaultRepo: string | null): IssueRef[] {
  const seen = new Map<string, IssueRef>();
  const [defaultOwner, defaultRepoName] = (defaultRepo ?? '').split('/');

  function labelFor(owner: string, repo: string, number: number): string {
    if (defaultOwner && defaultRepoName && owner === defaultOwner && repo === defaultRepoName) {
      return `#${number}`;
    }
    return `${repo}#${number}`;
  }

  for (const m of messages) {
    const text = m.text;
    if (!text) continue;

    for (const match of text.matchAll(URL_RE)) {
      const [, owner, repo, kind, n] = match;
      const number = Number(n);
      pushUnique(seen, {
        owner: owner!,
        repo: repo!,
        number,
        kind: kind as IssueKind,
        url: `https://github.com/${owner}/${repo}/${kind}/${number}`,
        label: labelFor(owner!, repo!, number)
      });
    }

    for (const match of text.matchAll(CROSS_RE)) {
      const [, owner, repo, n] = match;
      // Skip if this overlapped with a URL match above. The URL_RE is
      // greedy enough to swallow them, but matchAll runs independently
      // so we filter false positives by requiring the owner/repo to
      // look like a real slug (no dots-only, no leading digits).
      if (!/^[A-Za-z][\w.-]*$/.test(owner!) || !/^[A-Za-z][\w.-]*$/.test(repo!)) continue;
      const number = Number(n);
      pushUnique(seen, {
        owner: owner!,
        repo: repo!,
        number,
        kind: 'unknown',
        url: `https://github.com/${owner}/${repo}/issues/${number}`,
        label: labelFor(owner!, repo!, number)
      });
    }

    if (defaultOwner && defaultRepoName) {
      for (const match of text.matchAll(BARE_RE)) {
        const [, n] = match;
        const number = Number(n);
        pushUnique(seen, {
          owner: defaultOwner,
          repo: defaultRepoName,
          number,
          kind: 'unknown',
          url: `https://github.com/${defaultOwner}/${defaultRepoName}/issues/${number}`,
          label: `#${number}`
        });
      }
    }
  }

  return [...seen.values()].sort((a, b) => a.number - b.number);
}
