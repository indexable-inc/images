// Pure helpers over `git status --porcelain` output, shared by the base UX
// extensions. Kept pure so they can be unit-tested without git.
//
// Adapted from davis7dotsh/my-pi-setup (MIT, (c) 2026 Benjamin Davis).

// Count entries that have unstaged or untracked changes. Porcelain v1 lines
// are two status columns then a space: untracked is "??", and any non-space
// second column means the working tree differs from the index.
export function countUnstagedFiles(statusOutput) {
  if (statusOutput.length === 0) return 0;
  let count = 0;
  for (const line of statusOutput.split("\n")) {
    if (line.length < 2) continue;
    if (line.startsWith("??") || line[1] !== " ") count += 1;
  }
  return count;
}

// Extract the set of paths from porcelain output. Rename/copy entries look
// like `old -> new`; the destination is what we want. Quoted paths are
// unquoted naively (good enough for display/select purposes).
export function parseStatusPaths(statusOutput) {
  const files = new Set();
  for (const line of statusOutput.split("\n")) {
    if (line.length < 4) continue;
    const rawPath = line.slice(3).trim();
    if (!rawPath) continue;
    const targetPath = rawPath.includes(" -> ") ? rawPath.split(" -> ").at(-1) : rawPath;
    if (!targetPath) continue;
    files.add(targetPath.replace(/^"|"$/g, ""));
  }
  return files;
}
