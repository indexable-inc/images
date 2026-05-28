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

/// Keep both the head and tail of long strings visible. Most activity rows are
/// file paths and the tail (filename) is more identifying than the prefix.
export function middleTruncate(text: string, max: number): string {
  if (text.length <= max) return text;
  const head = Math.ceil((max - 1) / 2);
  const tail = Math.floor((max - 1) / 2);
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`;
}

/// Nix tags many real activities with type `unknown` (code 0). The text usually
/// leads with an action verb ("evaluating", "copying", "querying",
/// "downloading"). Synthesize the kind label from that verb so the column
/// actually classifies the row instead of repeating "unknown".
export function activityKind(typeName: string, text: string): string {
  if (typeName !== 'unknown') return typeName;
  const verb = /^([a-zA-Z]+)/.exec(text)?.[1];
  return verb === undefined ? 'note' : verb.toLowerCase();
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

/// Human byte-rate in decimal units (`kB`/`MB` are 1000-based, matching how
/// substituters and browsers report download speed). One decimal below 100 so a
/// `2.4 MB/s` reading stays legible; whole numbers above to avoid noise.
export function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return '0 B/s';
  const units = ['B/s', 'kB/s', 'MB/s', 'GB/s'];
  let value = bytesPerSecond;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const text = unit === 0 || value >= 100 ? String(Math.round(value)) : value.toFixed(1);
  return `${text} ${units[unit]}`;
}
