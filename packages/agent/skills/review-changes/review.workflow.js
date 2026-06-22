export const meta = {
  name: 'review-changes',
  description:
    'Multi-agent adversarial review of the working-tree diff: one code-reviewer finder per dimension, then an independent skeptic that tries to refute each finding.',
  phases: [
    { title: 'Review', detail: 'one code-reviewer per dimension over the git diff' },
    { title: 'Verify', detail: 'an independent skeptic tries to refute each finding' },
  ],
}

// The skill passes the repo root; default to the session cwd if it does not.
// Workaround: the Workflow tool delivers `args` as a JSON-encoded STRING even
// when the caller passes a real object, so `args.cwd` reads undefined and the
// review silently targets the wrong directory. Parse it back until
// https://github.com/anthropics/claude-code/issues/67627 is fixed.
const parsedArgs = typeof args === 'string' ? JSON.parse(args) : args
const cwd = parsedArgs && parsedArgs.cwd ? parsedArgs.cwd : '.'

// Surfaced once so the finder and the verifier read the exact same change.
// HEAD is shown ONLY when the working tree is clean (the change was already
// committed); otherwise reviewing HEAD would pull the previous, unrelated commit
// into every review. cwd is single-quoted to tolerate spaces/$/backticks.
const diffCmd =
  `cd '${cwd}' && ` +
  `if git --no-pager diff --quiet && git --no-pager diff --staged --quiet; then ` +
  `echo '(working tree clean; reviewing the last commit)' && git --no-pager show HEAD; ` +
  `else git --no-pager diff && git --no-pager diff --staged; fi`

const FINDING_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        properties: {
          category: {
            type: 'string',
            enum: ['correctness', 'security', 'performance', 'maintainability'],
          },
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          file: { type: 'string' },
          line: { type: 'string' },
          title: { type: 'string' },
          triggering_condition: { type: 'string' },
          fix: { type: 'string' },
        },
        required: ['category', 'severity', 'file', 'line', 'title', 'triggering_condition', 'fix'],
      },
    },
  },
  required: ['findings'],
}

const VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    is_real: { type: 'boolean' },
    confidence: { type: 'string', enum: ['high', 'medium', 'low'] },
    reason: { type: 'string' },
  },
  required: ['is_real', 'confidence', 'reason'],
}

const DIMENSIONS = [
  {
    key: 'correctness',
    focus:
      'Correctness only: boundary values, off-by-one, concurrency and races, error/cancellation/early-return paths, integer or float issues, broken contracts or invariants, and logic that tests would pass but is still wrong.',
  },
  {
    key: 'security',
    focus:
      'Security only (OWASP/CWE): injection, broken access control / IDOR, authn/authz gaps, secret or PII exposure, weak crypto or predictable randomness, unsafe deserialization, SSRF, path traversal. Give a CWE id and a one-line exploit per finding.',
  },
  {
    key: 'performance',
    focus:
      'Performance only: accidental O(n^2)+ work, N+1 queries or calls in a loop, allocation or I/O in a hot path, missing batching or caching, unbounded growth, and leaks. State the data volume at which it bites.',
  },
  {
    key: 'maintainability',
    focus:
      'Maintainability only: intent-hiding names, dead or unreachable code, real duplication of an existing helper, weakened types, comments that narrate the code, and violations of the repo conventions (cite the rule).',
  },
]

const findPrompt = (d) =>
  `Review the current change in ${cwd}. Get the diff with:\n  ${diffCmd}\n` +
  `Then read the full files around each hunk for context, and the repo's CLAUDE.md / AGENTS.md so findings match its real conventions.\n\n` +
  `Restrict your review to ONE dimension: ${d.focus}\n\n` +
  `Report only real, evidence-backed defects, each with an exact file:line, the concrete triggering input or condition, and a one-line fix. ` +
  `Do not pad; do not bikeshed. If there are no defects in this dimension, return an empty findings array.`

const verifyPrompt = (f) =>
  `An independent reviewer flagged this finding on the change in ${cwd}:\n\n` +
  `  [${f.category}/${f.severity}] ${f.file}:${f.line} - ${f.title}\n` +
  `  Triggering condition: ${f.triggering_condition}\n` +
  `  Proposed fix: ${f.fix}\n\n` +
  `Your job is to REFUTE it. Get the diff with:\n  ${diffCmd}\n` +
  `Read the actual code around ${f.file}:${f.line} and decide whether this defect can really occur on a reachable path. ` +
  `Default to is_real=false unless you can concretely confirm the triggering path; be skeptical of false positives.`

phase('Review')
const reviews = await pipeline(
  DIMENSIONS,
  (d) =>
    agent(findPrompt(d), {
      label: `find:${d.key}`,
      phase: 'Review',
      agentType: 'code-reviewer',
      schema: FINDING_SCHEMA,
    }).then((r) => (r && Array.isArray(r.findings) ? r.findings : [])),
  (findings) =>
    parallel(
      findings.map((f) => () =>
        agent(verifyPrompt(f), {
          label: `verify:${f.category}:${f.file}`,
          phase: 'Verify',
          schema: VERDICT_SCHEMA,
        }).then((v) => ({ ...f, verdict: v }))
      )
    )
)

const confirmed = reviews
  .flat()
  .filter(Boolean)
  .filter((f) => f.verdict && f.verdict.is_real)

const rank = { critical: 0, high: 1, medium: 2, low: 3 }
confirmed.sort((a, b) => rank[a.severity] - rank[b.severity])

const isBlocker = (f) => f.category === 'correctness' || f.category === 'security'
const blockers = confirmed.filter(isBlocker)
const warnings = confirmed.filter((f) => !isBlocker(f))

return {
  cwd,
  total_confirmed: confirmed.length,
  verdict: blockers.length > 0 ? 'BLOCK' : warnings.length > 0 ? 'approve-with-fixes' : 'approve',
  blockers,
  warnings,
}
