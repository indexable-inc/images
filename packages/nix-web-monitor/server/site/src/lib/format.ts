/// Split a Nix store path / derivation name into the leading content hash
/// (everything up to and including the first `-`) and the human-meaningful
/// `name-version[.drv]` remainder. The hash is opaque noise to readers; the
/// UI dims it so the actual package name jumps out.
///
/// Returns `{ hash: '', name: input }` when the input has no `-`, which is
/// not a real Nix path but is the right fallback for any oddball name.
export type DerivationParts = Readonly<{
  hash: string;
  name: string;
}>;

export function splitDerivation(path: string): DerivationParts {
  const slash = path.lastIndexOf('/');
  const base = slash === -1 ? path : path.slice(slash + 1);
  const dash = base.indexOf('-');
  if (dash === -1) return { hash: '', name: base };
  return {
    hash: `${base.slice(0, dash)}-`,
    name: base.slice(dash + 1)
  };
}

/// Compact duration formatter: `42s`, `3m04s`, `2h11m`, `1d04h`. Sub-second
/// resolution would just flicker; the UI ticks once per second anyway.
export function formatDuration(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  if (totalSeconds < 60) return `${String(totalSeconds)}s`;

  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes < 60) return `${String(minutes)}m${String(seconds).padStart(2, '0')}s`;

  const hours = Math.floor(minutes / 60);
  const remMinutes = minutes % 60;
  if (hours < 24) return `${String(hours)}h${String(remMinutes).padStart(2, '0')}m`;

  const days = Math.floor(hours / 24);
  const remHours = hours % 24;
  return `${String(days)}d${String(remHours).padStart(2, '0')}h`;
}
