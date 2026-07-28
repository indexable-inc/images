import { BILLING, type UsageStats } from './stats';

// Per-resource rates let the breakdown row render `$rate × value = bill`
// independently of the server's totalCost. The resource-monitor module's writer
// computes the same formulas against the same vm-config.json, so the rows sum
// to the server-published total.
const SECONDS_PER_HOUR = 60 * 60;
const BILLING_MONTH_SECONDS = 30 * 24 * SECONDS_PER_HOUR;

export const CPU_USD_PER_SECOND = BILLING.cpuUsdPerVcpuMonth / BILLING_MONTH_SECONDS;
export const MEM_USD_PER_GIB_SECOND =
  (BILLING.memoryUsdPerGibHour / SECONDS_PER_HOUR) * BILLING.marginMultiplier;
export const DISK_USD_PER_TIB_SECOND =
  (BILLING.storageUsdPerTibHour / SECONDS_PER_HOUR) * BILLING.marginMultiplier;

export const BAR_WIDTH = 20;

export type ResourceKey = 'cpu' | 'mem' | 'disk';

export type ResourceRow = {
  key: ResourceKey;
  label: string;
  ariaLabel: string;
  fraction: number;
  display: string;
  rateLabel: string;
  breakdownValue: string;
  bill: number;
};

// Log scale matches the reference dashboard: tiny values (1 GiB out of 1 PiB)
// remain visible on a 20-char bar instead of rounding to empty.
const LOG_GAMMA = 2;
const MIN_CPU = 0.01;
const MIN_MEM_GIB = 0.1;
const MIN_DISK_TIB = 1 / 1024;

export function logFraction(value: number, min: number, max: number): number {
  const clamped = Math.max(min, Math.min(max, value));
  const raw = (Math.log(clamped) - Math.log(min)) / (Math.log(max) - Math.log(min));
  return Math.pow(raw, LOG_GAMMA);
}

export function bar(fraction: number): string {
  const filled = Math.max(0, Math.min(BAR_WIDTH, Math.round(fraction * BAR_WIDTH)));
  return '█'.repeat(filled) + '░'.repeat(BAR_WIDTH - filled);
}

export function fmtUsdPerSecond(value: number): string {
  const abs = Math.abs(value);
  let decimals: number;
  if (abs >= 100) decimals = 0;
  else if (abs >= 10) decimals = 1;
  else if (abs >= 1) decimals = 2;
  else if (abs >= 0.01) decimals = 4;
  else decimals = 6;
  return (
    '$' +
    value.toLocaleString('en-US', {
      minimumFractionDigits: decimals,
      maximumFractionDigits: decimals
    }) +
    '/s'
  );
}

function fmtCores(value: number): string {
  return value.toLocaleString('en-US', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2
  });
}

function fmtPercent(value: number): string {
  const decimals = value < 0.01 ? 4 : value < 1 ? 2 : 1;
  return `${value.toLocaleString('en-US', {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals
  })}%`;
}

function fmtBytesNum(bytes: number): { value: string; unit: string } {
  const gib = bytes / 1024 ** 3;
  if (gib < 1024) {
    const digits = gib < 10 ? 2 : 1;
    return { value: gib.toFixed(digits), unit: 'GiB' };
  }
  const tib = gib / 1024;
  if (tib < 1024) {
    const digits = tib < 10 ? 2 : 1;
    return { value: tib.toFixed(digits), unit: 'TiB' };
  }
  return { value: (tib / 1024).toFixed(2), unit: 'PiB' };
}

function fmtTotalBytes(bytes: number): string {
  const tib = bytes / 1024 ** 4;
  if (tib === 1024) return '1 PiB';
  if (tib >= 1) return `${tib.toLocaleString('en-US')} TiB`;
  const gib = bytes / 1024 ** 3;
  return `${gib.toLocaleString('en-US')} GiB`;
}

export function resourceRows(stats: UsageStats): ResourceRow[] {
  const memGiB = stats.memory.usedBytes / 1024 ** 3;
  const diskTiB = stats.disk.usedBytes / 1024 ** 4;

  const memUsed = fmtBytesNum(stats.memory.usedBytes);
  const diskUsed = fmtBytesNum(stats.disk.usedBytes);

  const cpuFrac = logFraction(
    Math.max(stats.cpu.usedCores, MIN_CPU),
    MIN_CPU,
    stats.cpu.totalCores
  );
  const memFrac = logFraction(
    Math.max(memGiB, MIN_MEM_GIB),
    MIN_MEM_GIB,
    stats.memory.totalBytes / 1024 ** 3
  );
  const diskFrac = logFraction(
    Math.max(diskTiB, MIN_DISK_TIB),
    MIN_DISK_TIB,
    stats.disk.totalBytes / 1024 ** 4
  );

  return [
    {
      key: 'cpu',
      label: 'CPU',
      ariaLabel: 'CPU usage',
      fraction: cpuFrac,
      display: `${fmtCores(stats.cpu.usedCores)} / ${String(stats.cpu.totalCores)} vCPU (${fmtPercent(stats.cpu.percent)})`,
      rateLabel: fmtUsdPerSecond(CPU_USD_PER_SECOND),
      breakdownValue: `${fmtCores(stats.cpu.usedCores)} vCPU`,
      bill: stats.cpu.usedCores * CPU_USD_PER_SECOND
    },
    {
      key: 'mem',
      label: 'MEM',
      ariaLabel: 'memory usage',
      fraction: memFrac,
      display: `${memUsed.value} ${memUsed.unit} / ${fmtTotalBytes(stats.memory.totalBytes)} (${fmtPercent(stats.memory.percent)})`,
      rateLabel: fmtUsdPerSecond(MEM_USD_PER_GIB_SECOND),
      breakdownValue: `${memUsed.value} ${memUsed.unit}`,
      bill: memGiB * MEM_USD_PER_GIB_SECOND
    },
    {
      key: 'disk',
      label: 'DISK',
      ariaLabel: 'disk usage',
      fraction: diskFrac,
      display: `${diskUsed.value} ${diskUsed.unit} / ${fmtTotalBytes(stats.disk.totalBytes)} (${fmtPercent(stats.disk.percent)})`,
      rateLabel: fmtUsdPerSecond(DISK_USD_PER_TIB_SECOND),
      breakdownValue: `${diskUsed.value} ${diskUsed.unit}`,
      bill: diskTiB * DISK_USD_PER_TIB_SECOND
    }
  ];
}

const MIN_TOTAL_USD = MIN_CPU * CPU_USD_PER_SECOND;

export function totalBarFraction(totalUsd: number, maxTotalUsd: number): number {
  return logFraction(Math.max(totalUsd, MIN_TOTAL_USD), MIN_TOTAL_USD, maxTotalUsd);
}

export function maxTotalUsd(stats: UsageStats): number {
  return (
    stats.cpu.totalCores * CPU_USD_PER_SECOND +
    (stats.memory.totalBytes / 1024 ** 3) * MEM_USD_PER_GIB_SECOND +
    (stats.disk.totalBytes / 1024 ** 4) * DISK_USD_PER_TIB_SECOND
  );
}
