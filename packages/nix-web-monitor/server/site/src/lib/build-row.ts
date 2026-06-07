/// Shared helpers for rendering one build row, used by both the flat list and
/// the dependency tree so the two views format a build identically.

import { formatDuration } from '$lib/format';
import type { BuildNode } from '$lib/types';

/// Where a build ran. An empty or absent host is the local machine.
export function whereLabel(host: string | null): string {
  return host === null || host.length === 0 ? 'local' : host;
}

export function isRemote(host: string | null): boolean {
  return host !== null && host.length > 0;
}

/// Wall time a build has taken: live against `now` while running, frozen at its
/// finish once stopped. Never negative.
export function elapsedMs(build: BuildNode, now: number): number {
  return Math.max(0, (build.stoppedAtMs ?? now) - build.startedAtMs);
}

/// Duration column text. Planned rows have not started, so their elapsed time is
/// meaningless and the column stays blank until the build is live or done.
export function durationLabel(build: BuildNode, now: number): string {
  return build.status === 'planned' ? '' : formatDuration(elapsedMs(build, now));
}
