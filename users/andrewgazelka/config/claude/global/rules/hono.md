---
paths: "**/*.ts"
---

# Hono

Hono is a small, fast web framework for edge runtimes. Works on Cloudflare Workers, Bun, Deno, and Node.js.

## Quick Start

```typescript
import { Hono } from "hono";

const app = new Hono();

app.get("/", (c) => c.text("Hello Hono!"));
app.get("/json", (c) => c.json({ message: "Hello" }));

export default app;
```

## Routing

```typescript
// Path parameters
app.get("/users/:id", (c) => {
  const id = c.req.param("id");
  return c.json({ id });
});

// Query parameters
app.get("/search", (c) => {
  const query = c.req.query("q");
  return c.json({ query });
});
```

## Context (c)

```typescript
app.get("/", async (c) => {
  // Request
  c.req.path;
  c.req.query("key");
  c.req.param("id");
  c.req.header("X-Custom");
  await c.req.json();

  // Response
  return c.text("Hello");
  return c.json({ data: 1 });
  return c.redirect("/other");

  // Variables (type-safe)
  c.set("user", user);
  c.get("user");
});
```

## Middleware

```typescript
import { cors } from "hono/cors";
import { logger } from "hono/logger";

app.use("*", logger());
app.use("*", cors());

// Custom middleware
app.use("*", async (c, next) => {
  const start = Date.now();
  await next();
  c.header("X-Response-Time", `${Date.now() - start}ms`);
});
```

## Validation with Zod

```typescript
import { zValidator } from "@hono/zod-validator";
import { z } from "zod";

const schema = z.object({
  name: z.string(),
  age: z.number(),
});

app.post("/users", zValidator("json", schema), (c) => {
  const data = c.req.valid("json"); // typed!
  return c.json(data);
});
```

## Error Handling

```typescript
import { HTTPException } from "hono/http-exception";

app.get("/protected", (c) => {
  if (!authorized) {
    throw new HTTPException(401, { message: "Unauthorized" });
  }
  return c.json({ secret: "data" });
});

app.onError((err, c) => {
  if (err instanceof HTTPException) {
    return err.getResponse();
  }
  return c.json({ error: "Internal Server Error" }, 500);
});
```

## Typed Hono (RPC)

```typescript
const app = new Hono()
  .get("/users", (c) => c.json([{ id: 1, name: "John" }]))
  .post("/users", async (c) => {
    const body = await c.req.json();
    return c.json({ id: 2, ...body }, 201);
  });

export type AppType = typeof app;

// Client (type-safe!)
import { hc } from "hono/client";
const client = hc<AppType>("http://localhost:3000");
const res = await client.users.$get();
```

## Environment Variables

```typescript
type Bindings = {
  DATABASE_URL: string;
  API_KEY: string;
};

const app = new Hono<{ Bindings: Bindings }>();

app.get("/", (c) => {
  const dbUrl = c.env.DATABASE_URL; // typed!
  return c.text("OK");
});
```
