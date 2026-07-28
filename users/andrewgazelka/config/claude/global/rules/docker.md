---
paths: "**/Dockerfile, **/*.dockerfile, **/docker-compose.yml, **/docker-compose.yaml"
---

# Docker Best Practices

## Always Validate (CRITICAL)

**ALWAYS build the Dockerfile to confirm it works before considering the task complete.**

```bash
docker build -t myapp .
```

## BuildKit Cache Mounts (Always Use)

Use BuildKit cache mounts for package managers and build artifacts:

```dockerfile
# syntax=docker/dockerfile:1

# Rust - cache cargo registry and target directory
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release

# Copy binary OUT of cached target dir (cached dirs don't persist in image)
RUN --mount=type=cache,target=/app/target \
    cp /app/target/release/myapp /usr/local/bin/
```

### Why Cache Mounts Beat Dummy Files

**Anti-pattern - dummy source files for dependency caching:**
```dockerfile
# DON'T DO THIS
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
COPY src/ src/
RUN touch src/main.rs && cargo build --release
```

**Do this instead - BuildKit cache mounts:**
```dockerfile
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release && \
    cp /app/target/release/myapp /usr/local/bin/
```

## Multi-Stage Builds

Always use multi-stage for minimal final images:

```dockerfile
# syntax=docker/dockerfile:1

FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release && \
    cp /app/target/release/myapp /usr/local/bin/

# Minimal runtime image
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/bin/myapp /usr/local/bin/
CMD ["myapp"]
```

## .dockerignore

Always create `.dockerignore`:

```
target/
.git/
*.md
.gitignore
.env
.cargo/
```

## Extracting Binaries

```bash
docker build --target output --output type=local,dest=./out .
```

## Compose Watch (Hot Reload)

Use `docker compose watch` for development hot reload (Compose 2.22.0+):

```yaml
services:
  web:
    build: ./web
    develop:
      watch:
        - path: ./web/src
          action: sync          # Sync files without rebuild (for interpreted/HMR)
          target: /app/src
        - path: ./web/package.json
          action: rebuild       # Rebuild image on dependency changes

  backend:
    build: .
    develop:
      watch:
        - path: ./src
          action: rebuild       # Rebuild for compiled languages (Rust, Go)
        - path: ./Cargo.toml
          action: rebuild
```

**Actions:**
- `sync` - Copy files to container (use for JS/TS with HMR, static assets)
- `sync+restart` - Copy files and restart container (use for config files)
- `rebuild` - Rebuild image and recreate container (use for compiled languages)

**Usage:**
```bash
docker compose watch
```

## Reference Documentation

Full Docker docs: `~/.config/nix/claude/global/skills/docker/docs/`

- `docs/content/manuals/build/` - BuildKit, multi-stage, cache mounts
- `docs/content/manuals/compose/` - Compose reference
- `docs/content/reference/` - CLI and Dockerfile reference
