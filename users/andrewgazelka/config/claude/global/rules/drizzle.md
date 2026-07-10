---
paths: "**/drizzle.config.ts, **/drizzle/**"
---

# Drizzle ORM

Lightweight, type-safe TypeScript ORM with SQL-like API.

## Schema Example

```typescript
import { pgTable, serial, text, integer, timestamp } from 'drizzle-orm/pg-core'

export const users = pgTable('users', {
  id: serial('id').primaryKey(),
  email: text('email').notNull().unique(),
  name: text('name'),
  createdAt: timestamp('created_at').defaultNow()
})

export const posts = pgTable('posts', {
  id: serial('id').primaryKey(),
  title: text('title').notNull(),
  authorId: integer('author_id').references(() => users.id)
})
```

## Common Commands

```bash
npx drizzle-kit generate   # Generate migrations
npx drizzle-kit migrate    # Apply migrations
npx drizzle-kit push       # Push schema (no migration files)
npx drizzle-kit studio     # Open Drizzle Studio
npx drizzle-kit introspect # Introspect existing database
```

## drizzle.config.ts

```typescript
import { defineConfig } from 'drizzle-kit'

export default defineConfig({
  schema: './src/db/schema.ts',
  out: './drizzle',
  dialect: 'postgresql',
  dbCredentials: {
    url: process.env.DATABASE_URL!
  }
})
```

## Client Usage

```typescript
import { drizzle } from 'drizzle-orm/node-postgres'
import { eq, and, like } from 'drizzle-orm'
import { users, posts } from './schema'

const db = drizzle(process.env.DATABASE_URL!)

// Select
const allUsers = await db.select().from(users)
const user = await db.select().from(users).where(eq(users.id, 1))

// Insert
await db.insert(users).values({ email: 'alice@example.com', name: 'Alice' })

// Update
await db.update(users).set({ name: 'Updated' }).where(eq(users.id, 1))

// Delete
await db.delete(users).where(eq(users.id, 1))

// Join
const usersWithPosts = await db
  .select()
  .from(users)
  .leftJoin(posts, eq(users.id, posts.authorId))
```
