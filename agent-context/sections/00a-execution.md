---
name: execution
disclosure: always
---

## Execution

Run everything through the index Python MCP kernel (`python_exec`): it is the
default path for shell-outs, file reads, and any code you run, and the namespace
persists so helpers you define stay reusable. Reach for the Bash tool only when
the Python MCP is *completely* wedged — the event loop is frozen and neither
`kernel_trace` nor a fresh `python_exec` can recover it. Bash is the fallback
for an unresponsive kernel, not a parallel way to run ordinary commands.
