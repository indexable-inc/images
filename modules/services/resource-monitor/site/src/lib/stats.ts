import * as v from 'valibot';
import vmConfig from './vm-config.json';

export type Status = 'connecting' | 'live' | 'waiting';

// The wire payload only carries finite numbers; reject NaN and Infinity so a
// malformed reading falls back instead of rendering garbage.
const finiteNumber = v.pipe(v.number(), v.finite());

const cpuStatsSchema = v.object({
  usedCores: finiteNumber,
  totalCores: finiteNumber,
  percent: finiteNumber
});

const byteStatsSchema = v.object({
  usedBytes: finiteNumber,
  totalBytes: finiteNumber,
  percent: finiteNumber
});

export const usageStatsSchema = v.object({
  generatedAt: v.nullable(v.string()),
  cpu: cpuStatsSchema,
  memory: byteStatsSchema,
  disk: byteStatsSchema,
  costPerSecondUsd: finiteNumber
});

export type UsageStats = v.InferOutput<typeof usageStatsSchema>;

// default.nix generates vm-config.json from the module options before building
// the site, which keeps these values aligned with the Rust stats writer.
export const SERVER = vmConfig.server;
export const BILLING = vmConfig.billing;

export const FALLBACK_STATS: UsageStats = {
  generatedAt: null,
  cpu: { usedCores: 0, totalCores: SERVER.vcpu, percent: 0 },
  memory: { usedBytes: 0, totalBytes: SERVER.memoryGiB * 1024 ** 3, percent: 0 },
  disk: { usedBytes: 0, totalBytes: SERVER.storageTiB * 1024 ** 4, percent: 0 },
  costPerSecondUsd: 0
};

export function parseUsageStats(value: unknown): UsageStats | null {
  const result = v.safeParse(usageStatsSchema, value);
  return result.success ? result.output : null;
}
