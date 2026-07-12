---
paths: "**/*.ts, **/*.tsx"
---

# Upstash

Serverless data platform.

## Environment Variables

```bash
# QStash
QSTASH_TOKEN                  # API token for publishing
QSTASH_CURRENT_SIGNING_KEY    # For signature verification
QSTASH_NEXT_SIGNING_KEY       # For key rotation

# Redis
UPSTASH_REDIS_REST_URL        # Redis REST endpoint
UPSTASH_REDIS_REST_TOKEN      # Redis REST token
```

## Documentation Pointers

| Topic | Location |
|-------|----------|
| QStash scheduling | `./docs/qstash/features/schedules.mdx` |
| QStash queues | `./docs/qstash/features/queues.mdx` |
| QStash callbacks | `./docs/qstash/features/callbacks.mdx` |
| Redis SDK (TS) | `./docs/redis/sdks/ts/overview.mdx` |
| Redis commands | `./docs/redis/sdks/ts/commands/overview.mdx` |
| Workflow basics | `./docs/workflow/basics/` |

## Gotchas

### QStash Deduplication Window

**Deduplication window is only 10 minutes.** If your processing takes longer than 10 minutes, the next cron run will re-enqueue duplicates. For long-running jobs with cron triggers:

- Use Redis-based deduplication with longer TTLs
- Or use queues with appropriate parallelism settings
- Or increase cron interval beyond max processing time

## CLI Access

```bash
# QStash - list logs
http get -H { Authorization: $"Bearer ($env.QSTASH_TOKEN)" } "https://qstash.upstash.io/v2/logs"

# Redis - ping
http post -H { Authorization: $"Bearer ($env.UPSTASH_REDIS_REST_TOKEN)" } $"($env.UPSTASH_REDIS_REST_URL)/ping"
```
