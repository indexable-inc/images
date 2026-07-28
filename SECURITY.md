# Security policy

## Reporting a vulnerability

Email <andrew@ix.dev> with a description of the issue, a minimal reproduction, and any commit or release you have already tested against. Encrypt with [age](https://age-encryption.org) if the report contains exploit details (`age -r ...` recipient available on request).

Expect an acknowledgement within 3 business days. For confirmed issues that affect a released image or a fleet running on [ix](https://ix.dev), expect a fix or mitigation plan within 14 days; coordinated disclosure timelines for harder issues are set per-report.

Please do not open public GitHub issues for suspected vulnerabilities until a fix has shipped.

## Scope

In scope: code in this repository, images published to `registry.ix.dev/ix/*` from this repository, and the build-time supply chain that produces those images (workflows, lockfiles, fetched artifact catalogs, pinned actions).

Out of scope: the ix host platform itself (report to <security@ix.dev>), third-party Minecraft mods and plugins (report upstream first; we will track downstream impact), and security of agent-controlled workloads running inside ix VMs — see `CLAUDE.md` under "Trust model" for the threat model this repo assumes.

## Supported versions

Only `main` receives fixes. A fix lands on `main` after CI and artifact publication; release tags then name the exact commit so downstream consumers can pin a tag, OCI digest, or Nix store path instead of the moving `main` ref.
