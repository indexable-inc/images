/// Split a Nix store path / derivation name into the parts a reader actually
/// wants: the package `name`, its `version`, and the leading content `hash`. The
/// `.drv` suffix and store hash are noise, so the UI shows `name` prominently,
/// dims `version`, and drops the hash from the row (it stays in the row's
/// title for disambiguation).
///
/// The store base is `<hash>-<pname>-<version>.drv`; `pname` may itself contain
/// dashes, so the version is taken as the first `-<digit…>` run, matching Nix's
/// `pname-version` convention. Names with no version (e.g. `build-script-build`)
/// return an empty `version`. Returns the whole input as `name` for any oddball
/// string with no leading hash.
export type DerivationParts = Readonly<{
  hash: string;
  name: string;
  version: string;
}>;

export function splitDerivation(path: string): DerivationParts {
  const slash = path.lastIndexOf('/');
  const base = (slash === -1 ? path : path.slice(slash + 1)).replace(/\.drv$/, '');
  const dash = base.indexOf('-');
  if (dash === -1) return { hash: '', name: base, version: '' };

  const hash = `${base.slice(0, dash)}-`;
  const rest = base.slice(dash + 1);
  // First `-<digit>` starts the version; everything before it is the name.
  const versionAt = rest.search(/-\d/);
  if (versionAt === -1) return { hash, name: rest, version: '' };
  return {
    hash,
    name: rest.slice(0, versionAt),
    version: rest.slice(versionAt + 1)
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
  return `${formatBytes(bytesPerSecond)}/s`;
}

/// Human byte count in decimal units (`kB`/`MB` are 1000-based, matching how
/// Nix substituters and binary caches report path sizes). One decimal below 100
/// so `2.4 MB` stays legible; whole numbers above to keep the column quiet.
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'kB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const text = unit === 0 || value >= 100 ? String(Math.round(value)) : value.toFixed(1);
  return `${text} ${units[unit]}`;
}

/// Whether an activity's `progress` counters measure bytes or item counts. Nix
/// reports `copy_path` and `file_transfer` progress in bytes (data moving across
/// a store copy or substituter download); everything else (`copy_paths`,
/// `builds`, ...) counts items. The activities panel uses this to label a row as
/// `12.4 MB / 80 MB` versus `3 / 10`.
export type ProgressUnit = 'bytes' | 'count';

export function progressUnit(activityTypeName: string): ProgressUnit {
  return activityTypeName === 'file_transfer' || activityTypeName === 'copy_path'
    ? 'bytes'
    : 'count';
}
