# TypeScript SDK

```bash
bun add @indexable/sdk
```

Reads `IX_TOKEN` from the environment. Override with `{ token }`.

> [!NOTE]
> Node 20+, Bun, Deno, and modern browsers. Uses `await using` (TC39 explicit resource management), so target `ES2022`+.

## Sandbox

Opinionated surface over Client + Branch. The default way to use ix.

```ts
import { Sandbox } from "@indexable/sdk"

await using sbx = await Sandbox.oci("docker.io/library/python:3.12")
await sbx.exec(["ls", "-la"])        // fire-and-forget
```

Factories:

| | |
|---|---|
| `Sandbox.oci(image, opts?)` | Fully-qualified OCI image reference |
| `Sandbox.ubuntu(version?, opts?)` | `ubuntu:<version>` shorthand |
| `Sandbox.attach(vmId, opts?)` | Adopt an existing VM |

`SandboxOptions`: `token`, `baseUrl`, `region`, `env`, `name`, `ipv4`, `l7ProxyPorts`. Env fallbacks: `IX_TOKEN` → `IX_API_KEY`, `IX_API_BASE_URL`, `IX_REGION`.

`Sandbox.oci()` expects a fully-qualified OCI reference such as `docker.io/library/python:3.12`, `ghcr.io/owner/image:tag`, or `registry.example.com/team/app@sha256:...`.

## REPL

Stateful. Variables, cwd, env persist across `exec` calls on the same handle. Two handles are independent.

```ts
await using sbx = await Sandbox.oci("docker.io/library/python:3.12")

await using py = await sbx.repl("python")
await py.exec("x = 42")
await py.exec("print(x)")            // sees x

await using py2 = await sbx.repl("python")
await py2.exec("print(x)")           // NameError
```

Supported languages: `bash`, `python`, `node`, `bun`, `typescript`.

Bash persists shell state across calls:

```ts
await using sh = await sbx.repl("bash")
await sh.exec("cd /tmp && export FOO=1")
await sh.exec("pwd; echo $FOO")      // /tmp \n 1
```

`ReplResult = { output: string, exitCode: number }`. Output is combined stdout+stderr.

## Fork

```ts
const child = await sbx.fork("child-1")   // memory + disk, pennies
```

## Files

```ts
await sbx.write("/tmp/x", "hi")
const text = await sbx.read("/tmp/x")
const entries = await sbx.list("/tmp")
await sbx.writeBytes("/tmp/bin", new Uint8Array([1, 2, 3]))
const bytes = await sbx.readBytes("/tmp/bin")
```

## Lifecycle

`await using` deletes the VM when the scope exits. Call `sbx.close()` explicitly if you prefer.

```ts
await using sbx = await Sandbox.oci("docker.io/library/ubuntu:24.04")
// ... use sbx
// VM deleted here
```

## Low-level

`Client` and `Branch` expose the full API (regions, billing, tokens, volumes, migrations, observability). Use these when `Sandbox` is not enough.

```ts
import { Client } from "@indexable/sdk"
const ix = new Client({ token: process.env.IX_TOKEN!, baseUrl: "https://api.ix.dev" })
```
