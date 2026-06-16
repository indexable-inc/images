# profiles

`modules/profiles/` holds opt-in runtime profiles: cross-cutting NixOS config a
VM turns on once instead of repeating per service. Three modules are discovered
here (`profiles/base`, `profiles/jvm`, `profiles/extended-attributes`); each owns
its own option namespace. They are auto-discovered like any module (see
[common](../common.md)), but only `base` is auto-enabled.

## base (`ix.profiles.base`)

`modules/profiles/base/default.nix` ships the cross-cutting CLI and shell setup
every VM should have for debugging and introspection. It is auto-enabled by
`lib/image/oci-layer.nix:62` (`mkDefault true`), so it is on unless an image
opts out.

Option namespace: `ix.profiles.base` (`default.nix:17`).

- `enable` (`default.nix:18`).
- `shellWorkspace.enable` (bool, default true) - pre-create a writable workspace
  and auto-cd login shells into it (`default.nix:21`).
- `shellWorkspace.directory` (str, default `/work/ix`) (`default.nix:31`).

What it produces (`default.nix:39`):

- **Networking sysctls** (`default.nix:56-59`): BBR congestion control + `fq`
  qdisc, chosen for loss-insensitive throughput on residential last-miles that
  every inbound workload here faces (Minecraft players, Xpra clients, repo
  fetches). A no-op if `tcp_bbr` is absent.
- **Home Manager root config** (`default.nix:66`, Home Manager used as a NixOS
  module): Nushell as the login shell with `config.nu`/`login.nu`, plus
  `bash`/`zsh`/`fish` rc ownership so the integration flags below are not inert;
  `btop` (tuned gotham layout), `starship`, `atuin`, `zoxide`, `direnv`
  (nix-direnv), `fzf`, `mergiraf` (AST merge driver), `delta`, and a large `git`
  config (aliases, rebase/rerere/maintenance defaults). `env.nu` surfaces the
  workspace path as `$env.IX_WORKDIR`.
- **System shells + editor** (`default.nix:381`): `zsh`/`fish` NixOS modules and
  `neovim` wired through the NixOS module (config baked into the wrapper:
  treesitter with all grammars, telescope, gitsigns, which-key, oil, a Lua
  agent dispatcher, and the `ix-islands` colorscheme generated from
  `ix.islandsTheme`). `defaultEditor`, `vi`/`vim` aliases.
- **`environment.systemPackages`** (`default.nix:473`): debugging and editing
  CLI (`ast-grep`, `bat`, `bpftrace`, `gdb`, `lldb`, `strace`, `tcpdump`,
  `pahole`, `drgn`, `eu-*` from elifutils, `ripgrep`, `fd`, `mgrep`, `jq`,
  `nh`/`nix-tree`/`nix-output-monitor`, `zellij`, `helix`/`micro`, `gnutar`/
  `gzip`/`zstd`), plus the `ix.packages.mcp` engine.
- **tmpfiles** pre-create the workspace dir when `shellWorkspace.enable`
  (`default.nix:550`).

## jvm (`ix.profiles.jvm`)

`modules/profiles/jvm/default.nix` is the runtime side of the JVM toolchain:
opt-in, ships a JRE on PATH and sets `JAVA_HOME` so a VM that exists to run a
`.jar` (Minecraft, Velocity, Minestom) does not repeat the boilerplate. Build-time
helpers (`ix.languages.java.{jdk,maven,gradle}`) stay separate.

Option namespace: `ix.profiles.jvm` (`default.nix:20`).

- `enable` (`default.nix:21`).
- `package` (package) - JRE added to `systemPackages` and pointed at by
  `JAVA_HOME`; default is the Temurin JRE pinned in
  `lib/languages/jvm-defaults.nix`, the same major the Minecraft/Minestom/Velocity
  service modules default to, so an image and a service share one store path
  (`default.nix:23`).

Config sets `environment.systemPackages = [ package ]` and
`environment.variables.JAVA_HOME` (`default.nix:43-47`).

## extended-attributes (`ix.extendedAttributes`)

`modules/profiles/extended-attributes/default.nix` applies filesystem extended
attributes during system activation. This is the option backing the
`ix.extendedAttributes` writes other modules make (e.g.
[minecraft](../minecraft/overview.md) stamps `user.ix.minecraft.*` on its data
dirs).

Option namespace: `ix.extendedAttributes` (`default.nix:107`), an attrset keyed
by absolute path; each entry is `{ create, attributes }` (`default.nix:16-30`):

- `create` (bool) - create the path as a directory first; missing paths are
  otherwise skipped (`default.nix:18`).
- `attributes` (attrs of str) - xattrs to set; names must use the `user.`
  namespace (`default.nix:24`).

Config (`default.nix:120`):

- **Assertions**: keys must be absolute paths without empty or `..` segments;
  attribute names must use the `user.*` namespace (`default.nix:121-130`).
- Adds `pkgs.attr` and registers
  `system.activationScripts.ix-extended-attributes` (`default.nix:132-134`),
  which runs `setfattr` per path. It gracefully skips paths on filesystems
  without xattr support and refuses to write through a symlink. This is metadata,
  not a containment boundary.

## How they are wired

All three are auto-discovered under `profiles/`. `base` is auto-enabled by the
OCI image layer; `jvm` and `extended-attributes` stay inert until enabled (or, for
extended-attributes, until another module writes a non-empty
`ix.extendedAttributes`).
