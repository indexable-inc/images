# ray

`modules/services/ray/default.nix` runs one Ray cluster spanning the tailnet,
plus the ix-mcp engine that drives it. It is the deployment side of the `fleet`
Python module bundled in ix-mcp (`packages/mcp`): `fleet.run`/`fleet.submit`
ship cloudpickled callables to Ray and the object store carries the data;
`fleet.in_kernel` runs code in a node's live ix-mcp session over `/api/exec`.

Option namespace: `services.ix-ray` (`default.nix:192`).

## Topology and repo-agnostic shape

One Ray cluster. Exactly one node sets `role = "head"` (it holds the GCS); every
other node is `role = "worker"` pointing `headAddress` at the head's tailscale
IP. Daemons bind their tailscale IPv4 (resolved at runtime), so the cluster
lives on the tailnet, which is the trust boundary (Ray has no per-call auth).

The module declares no `ix.*` NixOS options, so it imports into any NixOS
system. It takes the index lib through `_module.args.indexLib` (named, not `ix`,
because a host binds `ix` to its own specialArg) for
`writeNushellApplication`/`systemdHardening` (`default.nix:30-43`). The ix-mcp
engine is handed in via `notebookPackage`.

## Public surface (options)

- `enable`, `role` (`head`|`worker`, default `worker`), `headAddress` (nullable;
  required on workers, null on head) (`default.nix:193-216`).
- `package` (default `python3Packages.ray`) - the same Ray the ix-mcp
  interpreter imports, so cluster and kernels run identical versions
  (`default.nix:218`).
- `notebookPackage` (nullable package) - the ix-mcp package providing the
  `ix-notebook` engine binary; required when `notebook.enable`
  (`default.nix:226`).
- Pinned ports: `gcsPort` (6379), `clientServerPort` (10001, head only, the
  `ray://` endpoint `fleet.connect()` uses), `nodeManagerPort` (6380),
  `objectManagerPort` (6381), `workerPortLow`/`workerPortHigh` (10002-10031),
  `execPort` (8799, the ix-mcp `/api/exec` port, must match the `fleet` module's
  `IX_FLEET_EXEC_PORT`) (`default.nix:237-285`).
- `objectStoreMemory` (nullable bytes) - Plasma store size; null autodetects
  (~30% RAM). Spills to `/var/lib/ray/spill` when full (`default.nix:287`).
- `notebook.enable` (default true) - run the ix-mcp engine on this node
  (`default.nix:299`).
- `execTrustNetwork` (default true) - trust the tailnet as the `/api/exec` auth
  boundary; `tokenFile` adds a bearer token (`default.nix:309`, `:321`).
- `openFirewall` (default false) - open inter-node ports; usually unnecessary on
  a tailnet (`default.nix:333`).

## Key internals

- **Mode args** (`default.nix:74-89`): head gets `--head --port <gcsPort>
  --ray-client-server-port <clientServerPort> --include-dashboard false` (the web
  dashboard needs `ray[default]` extras nixpkgs omits); worker gets `--address
  <headAddress>:<gcsPort>`. Common args pin the node/object manager and worker
  ports, `--temp-dir /run/ray` (short path so the AF_UNIX plasma socket stays
  under the 108-byte `sun_path` limit), and the filesystem spill config.
- **Launchers** (`default.nix:115`, `:152`): Nushell apps that resolve this
  node's tailscale IPv4 at runtime, fail loudly if tailscale is down, then exec
  `ray start ... --node-ip-address <ip> --block`. The notebook launcher also sets
  `IX_MCP_HOST` and `RAY_ADDRESS` and execs `ix-notebook`.
- **Assertions** (`default.nix:346-359`): head must not set `headAddress`; a
  worker must; `notebook.enable` requires `notebookPackage`.

## What it produces

- `networking.firewall` (only when `openFirewall`): exec + node/object manager
  ports, worker range, and on the head the GCS + client-server ports
  (`default.nix:361-377`).
- `systemd.services.ix-ray` (`default.nix:379`): `indexLib.systemdHardening` +
  `DynamicUser`, `RuntimeDirectory`/`StateDirectory = ray`, after
  `tailscaled.service`. `PrivateDevices` and `PrivateUsers` are forced off so an
  attaching kernel can map the shared-memory object store.
- `systemd.services.ix-ray-notebook` (`default.nix:408`, when
  `notebook.enable`): the ix-mcp engine, requires `ix-ray.service`, same
  shared-memory exceptions.

## How it is wired

Auto-discovered as `services/ray`, but consumed standalone via `indexLib` +
`notebookPackage`. Runs `python3Packages.ray` and the ix-mcp engine from
`packages/mcp`.
