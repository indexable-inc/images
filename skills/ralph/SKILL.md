---
name: ralph
description: Run a task with Ralph
---

create ./.ralph/<task>/INSTRUCTION.md with that given instruction

then

while true:
    spawn a fresh general agent (not a fork — each starts with clean context) telling it to do ./.ralph/<task>/INSTRUCTION.md
