export const SCOPE_SEP = String.fromCharCode(0x1f); // matches dashboard::hub::SCOPE_SEP

export function paneScope(key: string): string {
  const sep = key.indexOf(SCOPE_SEP);
  return sep === -1 ? '' : key.slice(0, sep);
}

export function compareCreatedAt(a: number | undefined, b: number | undefined): number {
  if (a !== undefined && b !== undefined && a !== b) return a - b;
  if (a !== undefined && b === undefined) return -1;
  if (a === undefined && b !== undefined) return 1;
  return 0;
}
