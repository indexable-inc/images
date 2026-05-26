## Tests

Tests should protect behavior that can regress across boundaries: module merges,
generated units, fleet rendering, artifact wiring, security posture, and runtime
contracts. Avoid asserting facts already obvious from the literal config under
test.

Image and reusable package derivations expose focused tests through
`passthru.tests.<name>`. Cross-image eval invariants live in checks. Keep
`checkPhase` or `installCheckPhase` for cheap checks that should always run with
the build.

When a change tightens source filtering, dependency identity, generated
derivations, or cache behavior, add a test that changes one small input and
proves the unrelated output remains unchanged.

