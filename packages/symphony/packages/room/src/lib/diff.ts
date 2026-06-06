// Parse a unified diff into per-file FileDiffMetadata using Pierre's
// own parser. Pierre's `preloadFileDiff` then turns each entry into
// the fully styled split/unified hunk view that the diffs-container
// custom element wraps. Keep this in lib/ so the routing and store
// layer can ask "does this look like a diff" without importing the
// renderer.

import { parsePatchFiles, type FileDiffMetadata } from '@pierre/diffs';

export type DiffFile = FileDiffMetadata;

export function parseDiffFiles(patch: string): DiffFile[] {
  if (!patch || !patch.trim()) return [];
  try {
    const parsed = parsePatchFiles(patch);
    return parsed.flatMap((p) => p.files);
  } catch {
    return [];
  }
}
