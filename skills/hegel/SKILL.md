---
name: hegel
description: Property-based testing with Hegel (never proptest). Quick reference for API, generators, and links.
---

# Hegel — Property-Based Testing

**Never use `proptest`. Use `hegel` for all property-based testing.**

Hegel is built on Hypothesis (via a protocol bridge) and provides internal shrinking, a test database for replay, and high-quality generators out of the box.

## TLDR

- Annotate tests with `#[hegel::test]`, draw values with `tc.draw(generator)`.
- Compose custom generators with `#[hegel::composite]`.
- Internal shrinking means minimal failing examples with zero manual shrinker code.
- Requires Python at test time (Hypothesis runs as a backend server).

## Quick Example

```rust
#[hegel::test(test_cases = 1000)]
fn roundtrip(tc: hegel::TestCase) {
    let v: i64 = tc.draw(generators::integers());
    assert_eq!(v, MyType::decode(&MyType::encode(v)));
}
```

## Links

- **Rust crate (hegeltest):** https://github.com/hegeldev/hegel-rust
- **Core protocol / server:** https://github.com/hegeldev/hegel-core
- **crates.io:** https://crates.io/crates/hegeltest
- **Intro blog post:** https://antithesis.com/blog/2026/hegel/
- **Hypothesis (upstream engine):** https://hypothesis.readthedocs.io/

## When to Use

- Parsers and serialization roundtrips
- FFI boundary contracts
- Arbitrary-input invariants (never-panic, idempotency, commutativity)
- Model-based testing (compare optimized impl against simple reference)

## Key API Surface

| Item | Purpose |
|------|---------|
| `#[hegel::test(test_cases = N)]` | Entry point for a property test |
| `tc.draw(gen)` | Draw a value from a generator |
| `#[hegel::composite]` | Define a custom composite generator |
| `generators::integers()` | Integer generator (supports `.min_value()` / `.max_value()`) |
| `generators::text()` | String generator |
| `generators::booleans()` | Bool generator |
| `generators::vecs(gen)` | Vec generator (supports `.min_size()` / `.max_size()`) |
