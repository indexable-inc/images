// Deterministic scoring for beam-search branches.
//
// Ground truth decides, not a model. A branch that passes its score command
// (exit 0) always beats one that fails; among passers, the smaller diff wins
// (less churn for the same result). Pure so it is unit-testable.
export function scoreBranch({ exitCode, diffLines }) {
  const passed = exitCode === 0 ? 1 : 0;
  return passed * 1e6 - (diffLines ?? 0);
}

export function rank(branches) {
  return [...branches].sort((a, b) => b.score - a.score);
}
