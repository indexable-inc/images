// Generic 1-10 scoring vocabulary. A catalog (RFCs, packages, ...) declares
// its dimensions, scores each item with a value AND a why so rankings can be
// critiqued in review, and feeds ScoreChart.svelte.

export type Score = {
  value: number;
  why: string;
};

export type ScoreDimension = {
  key: string;
  label: string;
  // Axis endpoint words, low to high.
  low: string;
  high: string;
  // What the axis really measures; rendered as the legend.
  meaning: string;
};

// An item plotted on a ScoreChart. `label` is the short mark label (an RFC
// number, a package name); `colorVar` is a CSS custom property carrying the
// mark color, e.g. 'var(--status-draft)'.
export type ScoredItem = {
  id: string;
  label: string;
  title: string;
  detail: string;
  colorVar: string;
  href: string;
  scores: Record<string, Score>;
};

export function validateScores(
  owner: string,
  dimensions: readonly ScoreDimension[],
  scores: Record<string, Score> | undefined
): void {
  for (const { key } of dimensions) {
    const score = scores?.[key];
    if (score === undefined) {
      throw new Error(`${owner}: missing score '${key}'`);
    }
    if (!Number.isInteger(score.value) || score.value < 1 || score.value > 10) {
      throw new Error(`${owner}: score '${key}' must be an integer 1-10, got ${String(score.value)}`);
    }
    if (typeof score.why !== 'string' || score.why.length === 0) {
      throw new Error(`${owner}: score '${key}' needs a why so the number can be critiqued`);
    }
  }
}
