// Ground-truth helpers shared by the harnesses.
//
// The verdict and the branch score are decided by CODE, not by a model judging
// a model. The prosecutor/branch agent runs commands; these functions read the
// result. Keeping this pure makes the decision deterministic and testable.

// Parse a prosecutor's final message for its verdict line. A claim is only
// upheld on an explicit UPHELD with no BROKEN line - anything ambiguous fails
// closed (treated as broken), because the whole point is to not give the
// executor the benefit of the doubt.
export function parseVerdict(text) {
  const broken = text.match(/VERDICT:\s*BROKEN\s*(.*)/i);
  if (broken) {
    return { upheld: false, evidence: broken[1].trim() || "claim refuted" };
  }
  if (/VERDICT:\s*UPHELD/i.test(text)) {
    return { upheld: true, evidence: "" };
  }
  return { upheld: false, evidence: "prosecutor returned no clear verdict" };
}
