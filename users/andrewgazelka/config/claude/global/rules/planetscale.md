---
paths: "**/schema.prisma, **/*.sql"
---

# PlanetScale

MySQL-compatible serverless database.

## Connecting via MySQL CLI

```bash
mysql -h $DATABASE_HOST -u $DATABASE_USERNAME -p$DATABASE_PASSWORD $DATABASE_NAME --ssl-mode=REQUIRED
```

## Environment Variables

- `DATABASE_HOST` - PlanetScale host (e.g., `aws.connect.psdb.cloud`)
- `DATABASE_USERNAME` - Database user
- `DATABASE_PASSWORD` - Database password
- `DATABASE_NAME` - Database name
- `DATABASE_URL` - Full connection string (Prisma format)

## Read-Only by Default

**NEVER write to the database unless explicitly instructed.** Use SELECT queries only for debugging/inspection.

## Row Limit (100,000)

PlanetScale (Vitess) enforces a **maximum of 100,000 rows per query**. Queries exceeding this will fail with:

```
vttablet: rpc error: code = Aborted desc = Row count exceeded 100000
```

**Solution: Cursor-based pagination**

```typescript
const BATCH_SIZE = 50_000;
const results: T[] = [];
let cursor: string | undefined;

while (true) {
  const batch = await prisma.table.findMany({
    where: { ... },
    take: BATCH_SIZE,
    ...(cursor ? { skip: 1, cursor: { id: cursor } } : {}),
    orderBy: { id: "asc" },
  });

  if (batch.length === 0) break;

  results.push(...batch);
  cursor = batch[batch.length - 1].id;

  if (batch.length < BATCH_SIZE) break;
}
```

## Common Debug Queries

```sql
-- Check table structure
DESCRIBE table_name;

-- Count rows
SELECT COUNT(*) FROM table_name;

-- Sample recent rows
SELECT * FROM table_name ORDER BY created_at DESC LIMIT 10;
```
