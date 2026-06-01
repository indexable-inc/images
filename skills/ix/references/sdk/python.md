# Python SDK

```bash
uv add ix-sdk
```

Reads `IX_TOKEN` from the environment. Override with `token=...`.

> [!NOTE]
> Async-first. Python >= 3.11. Sync shim is minimal.

## Sandbox

```python
from ix_sdk import Sandbox

async with await Sandbox.oci("docker.io/library/python:3.12") as sbx:
    await sbx.exec(["ls", "-la"])            # fire-and-forget
```

Factories:

| | |
|---|---|
| `Sandbox.oci(image, **opts)` | Fully-qualified OCI image reference |
| `Sandbox.ubuntu(version="24.04", **opts)` | Ubuntu shorthand |
| `Sandbox.attach(vm_id, **opts)` | Adopt an existing VM |

Options: `token`, `base_url`, `region`, `env`, `name`, `ipv4`, `l7_proxy_ports`. Env fallbacks: `IX_TOKEN` / `IX_API_KEY`, `IX_API_BASE_URL`, `IX_REGION`.

`Sandbox.oci()` expects a fully-qualified OCI reference such as `docker.io/library/python:3.12`, `ghcr.io/owner/image:tag`, or `registry.example.com/team/app@sha256:...`.

## REPL

Stateful. Variables, cwd, env persist across `exec` calls. Independent handles do not share state.

```python
async with await Sandbox.oci("docker.io/library/python:3.12") as sbx:
    async with await sbx.repl("python") as py:
        await py.exec("x = 42")
        await py.exec("print(x)")            # sees x

    async with await sbx.repl("python") as py2:
        await py2.exec("print(x)")           # NameError
```

Languages: `"bash"`, `"python"`, `"node"`, `"bun"`, `"typescript"`.

```python
async with await sbx.repl("bash") as sh:
    await sh.exec("cd /tmp && export FOO=1")
    await sh.exec("pwd; echo $FOO")          # /tmp \n 1
```

`ReplResult` has `output: str` (combined stdout+stderr) and `exit_code: int`.

## Fork

```python
child = await sbx.fork("child-1")            # memory + disk, pennies
```

## Files

```python
await sbx.write("/tmp/x", "hi")
text = await sbx.read("/tmp/x")
entries = await sbx.list("/tmp")
await sbx.write_bytes("/tmp/bin", b"\x01\x02\x03")
data = await sbx.read_bytes("/tmp/bin")
```

## Lifecycle

`async with` deletes the VM on scope exit. Use `await sbx.close()` to manage the lifetime yourself.

## Low-level

`Client` / `Branch` expose the full API (regions, billing, tokens, volumes, migrations, observability). Reach for it when `Sandbox` is not enough.

```python
from ix_sdk import Client
ix = Client(token=os.environ["IX_TOKEN"])
```
