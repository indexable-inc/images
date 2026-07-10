---
paths: "**/*.rs, **/*.ts, **/Cargo.toml, **/package.json"
---

# OpenTelemetry Distributed Tracing

## Environment Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Collector endpoint | `http://collector:4317` (gRPC) or `:4318` (HTTP) |
| `OTEL_SERVICE_NAME` | Service name in traces | `api-v2`, `vectorizer-server` |

### Ports

- **4317**: gRPC (Rust default)
- **4318**: HTTP (TypeScript/browsers)

## Rust Setup (Axum)

### Dependencies

```toml
[dependencies]
axum-tracing-opentelemetry = "0.32"
init-tracing-opentelemetry = { version = "0.32", features = ["otlp", "tracing_subscriber_ext"] }
opentelemetry = "0.29"
opentelemetry-otlp = { version = "0.29", features = ["grpc-tonic"] }
opentelemetry_sdk = { version = "0.29", features = ["rt-tokio"] }
tracing = "0.1"
tracing-opentelemetry = "0.30"
```

### Minimal Setup

```rust
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};

#[tokio::main]
async fn main() {
    // CRITICAL: Set propagator BEFORE init_subscribers for distributed tracing
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    let _guard = init_tracing_opentelemetry::tracing_subscriber_ext::init_subscribers()
        .expect("Failed to initialize tracing");

    let app = Router::new()
        .route("/api", post(handler))
        .layer(OtelInResponseLayer::default())  // Adds traceparent to response
        .layer(OtelAxumLayer::default())         // Extracts traceparent from request
        .route("/health", get(health));          // Health outside trace layers
}
```

## TypeScript Setup (Bun/Node)

### Dependencies

```json
{
  "@opentelemetry/api": "^1.9.0",
  "@opentelemetry/core": "^1.30.0",
  "@opentelemetry/exporter-trace-otlp-http": "^0.57.0",
  "@opentelemetry/resources": "^1.30.0",
  "@opentelemetry/sdk-trace-base": "^1.30.0",
  "@opentelemetry/semantic-conventions": "^1.30.0"
}
```

### Minimal Setup

```typescript
import { trace, context, propagation } from "@opentelemetry/api";
import { BasicTracerProvider, BatchSpanProcessor } from "@opentelemetry/sdk-trace-base";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { Resource } from "@opentelemetry/resources";
import { ATTR_SERVICE_NAME } from "@opentelemetry/semantic-conventions";
import { W3CTraceContextPropagator } from "@opentelemetry/core";

export function initTelemetry(): void {
  const endpoint = process.env.OTEL_EXPORTER_OTLP_ENDPOINT;
  if (!endpoint) return;

  // CRITICAL: Set global propagator BEFORE registering provider
  propagation.setGlobalPropagator(new W3CTraceContextPropagator());

  const provider = new BasicTracerProvider({
    resource: new Resource({
      [ATTR_SERVICE_NAME]: process.env.OTEL_SERVICE_NAME || "my-service",
    }),
  });

  provider.addSpanProcessor(new BatchSpanProcessor(
    new OTLPTraceExporter({ url: `${endpoint}/v1/traces` })
  ));
  provider.register();
}
```

### Propagating Context in HTTP Calls

```typescript
const headers: Record<string, string> = { "Content-Type": "application/json" };
propagation.inject(context.active(), headers);  // Injects traceparent header

const response = await fetch(url, { method: "POST", headers, body });
```

## Distributed Tracing (Cross-Service)

For traces to link across services:

1. **Caller** injects `traceparent` header via `propagation.inject()`
2. **Callee** extracts it via middleware (OtelAxumLayer) or manually
3. Both services export to the **same collector**
