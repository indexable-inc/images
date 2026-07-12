---
paths: "**/wrangler.toml, **/wrangler.json"
---

# Cloudflare

## Workers

- Serverless JavaScript/Rust at the edge
- Use `wrangler` CLI for deployment
- **Wrangler is already logged in** - no need to run `wrangler login`
- **Prefer `wrangler.toml` over `wrangler.json`** for configuration

## Foundations Library (Rust)

Cloudflare's `foundations` (crate name: `rustfoundry`) is a modular Rust library for building production-grade distributed systems.

### Quick Start

```toml
[dependencies]
rustfoundry = "4"
```

```rust
use rustfoundry::{
    telemetry::{self, TelemetryConfig},
    BootstrapResult,
};

fn main() -> BootstrapResult<()> {
    let _telemetry = telemetry::init(TelemetryConfig::default())?;
    Ok(())
}
```

### Memory Profiling with jemalloc

```rust
use rustfoundry::memory;

memory::profiling::enable();
let profile = memory::profiling::dump_profile()?;
```

### Seccomp Sandboxing

```rust
use rustfoundry::security::seccomp;
seccomp::apply_filter(&allowed_syscalls)?;
```

## Other Products

| Product | Description |
|---------|-------------|
| **R2** | S3-compatible object storage, no egress fees |
| **KV** | Key-value storage at the edge, eventually consistent |
| **D1** | SQLite at the edge |
| **Queues** | Message queues for Workers, at-least-once delivery |
