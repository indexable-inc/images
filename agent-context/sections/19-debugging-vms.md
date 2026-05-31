---
name: debugging-vms
disclosure: progressive
description: "Debugging running ix VMs: ix ls/shell, rendered units, journals, nix run nixpkgs#tool. Use when a VM service is failing or you need to inspect a guest."
---

## Debugging VMs

Use the real ix CLI to inspect running VMs before inferring from source. Prefer
machine-readable host commands when available, such as `ix ls --output json`.

Run guest commands with `ix shell <vm> -- <cmd> ...`. If command lookup differs
from an interactive shell, use absolute paths from the guest.

For service failures, check the rendered unit and the live journal inside the
VM. Confirm the unit exists, PID 1 is systemd, and the process is failing after
launch before changing image or module code.

When a debugging tool is missing on the host or in the dev shell, run it through
nixpkgs with `nix run nixpkgs#<tool> -- ...` instead of hand-installing it.

