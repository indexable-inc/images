---
name: module-conventions
description: "NixOS module conventions: options vs config, auto-discovery, typed options, service families, port claims. Use when adding or editing a module under modules/."
---

## Module conventions

Modules declare options and config. Keep each module inert until its enable flag
or equivalent activation condition is set. Prefer independent modules picked up
through the auto-discovered registry over modules importing each other.

A new module is a directory at `modules/<category>/<name>/` with its own
`default.nix`. The walker in [`lib/default.nix`](lib/default.nix) (see
`discoverModules`) finds it on the next eval; no registry edit is needed. Nested
sub-modules follow the same shape (`modules/services/minecraft/fabric/default.nix`
becomes `nixosModules.minecraft.fabric`). Helper data that lives next to a
module but is not itself a NixOS module belongs in a sibling directory whose
name starts with `_`, which the walker skips.

Public options should describe the user's domain. Hide storage mechanics behind
typed options, generated files, and small adapters. Use broad escape hatches only
at true foreign-format boundaries and name that boundary in the description.

Structured config belongs in structured values. Prefer `pkgs.formats.*`,
freeform submodules, and typed option trees over string fragments that cannot
merge, inspect, or receive `mkDefault` and `mkForce` cleanly.

Cross-cutting helpers come through `specialArgs.ix` or the public flake `lib`
surface. Avoid relative-up imports that climb across repo layers. Child and
sibling paths inside one package or module directory are fine.

Service families share a runtime module plus variant modules that fill typed
slots. Enabling a variant should enable the runtime by default. Mutually
exclusive variants should fail loudly through module merging or explicit
validation.

Every module that binds a TCP or UDP socket should declare a port claim next to
the bind setting or firewall declaration. A duplicate claim in the same
namespace is a useful eval-time failure; intentional co-location needs a real
namespace boundary or an explicit alternate port.

Modules that manage artifacts should consume catalogs, lockfiles, or caller
supplied sources. Presets and examples should read like intent, with local or
private artifacts shown only when that is the point of the example.
