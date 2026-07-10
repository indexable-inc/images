---
paths: "**/schema.prisma, **/prisma/**"
---

# Prisma ORM

## Schema Example

```prisma
datasource db {
  provider = "postgresql"
  url      = env("DATABASE_URL")
}

generator client {
  provider = "prisma-client-js"
}

model User {
  id    Int     @id @default(autoincrement())
  email String  @unique
  name  String?
  posts Post[]
}

model Post {
  id       Int    @id @default(autoincrement())
  title    String
  author   User   @relation(fields: [authorId], references: [id])
  authorId Int
}
```

## Common Commands

```bash
npx prisma init           # Initialize Prisma
npx prisma generate       # Generate Prisma Client
npx prisma migrate dev --name init  # Create migration
npx prisma migrate deploy # Apply migrations (production)
npx prisma db push        # Push schema without migration
npx prisma studio         # Open Prisma Studio
npx prisma format         # Format schema
```

## Client Usage

```typescript
import { PrismaClient } from '@prisma/client'

const prisma = new PrismaClient()

// Create
const user = await prisma.user.create({
  data: { email: 'alice@example.com', name: 'Alice' }
})

// Read
const users = await prisma.user.findMany({
  where: { email: { contains: '@example.com' } },
  include: { posts: true }
})

// Update
await prisma.user.update({
  where: { id: 1 },
  data: { name: 'Updated Name' }
})

// Delete
await prisma.user.delete({ where: { id: 1 } })
```
