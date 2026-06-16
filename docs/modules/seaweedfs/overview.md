# seaweedfs

`modules/services/seaweedfs/default.nix` runs single-node S3-compatible object
storage. One `weed server -s3` process runs the master, volume, filer, and S3
gateway in a single binary, so a single node needs no orchestration (enabling
`-s3` auto-starts the filer it depends on). SeaweedFS is chosen as the fastest
single-node S3 surface in nixpkgs (Apache-2.0); the source note rejects MinIO
(AGPL, gutted OSS console) and Garage (AGPL, weaker on small objects).

Option namespace: `services.ix-seaweedfs` (`default.nix:54`).

## Public surface (options)

- `enable` (`default.nix:55`).
- `package` (default `pkgs.seaweedfs`) (`default.nix:57`).
- `port` (port, default 8333) - S3 gateway listen port (`default.nix:59`).
- `bindAddress` (str, default `0.0.0.0`) - listen address for every listener;
  exposure is bounded by the firewall, which only opens `port` (`default.nix:65`).
- `openFirewall` (bool, default true) (`default.nix:75`).
- `configFile` (nullable path) - SeaweedFS S3 identities config (`-s3.config`)
  with access/secret key pairs; point at a runtime secret so credentials never
  enter the store (`default.nix:81`).
- `allowAnonymous` (enable option) - explicit opt-in to unauthenticated S3
  access (`default.nix:93`).
- `extraArgs` (list of str) - appended to `weed server` (`default.nix:98`).

## Key internals

- The server argv pins `-dir=/var/lib/seaweedfs` (equal to the systemd
  `StateDirectory`, because `weed server` refuses to start unless `-dir` exists
  and is writable and does not create it), `-ip=127.0.0.1` (peers advertise
  loopback on one node), `-ip.bind=<bindAddress>`, `-s3`, `-s3.port=<port>`, plus
  optional `-s3.config` and `extraArgs` (`default.nix:42-51`).
- **Credentials assertion** (`default.nix:110-118`): refuses to evaluate unless
  `configFile != null` or `allowAnonymous` is set, so an unauthenticated S3
  endpoint is never the silent default.

## What it produces

- `ix.networking.portClaims.ix-seaweedfs` (tcp) + firewall gated by
  `openFirewall` (`default.nix:120-126`).
- `ix.healthChecks.ix-seaweedfs`: `curl --fail
  http://127.0.0.1:<port>/healthz`, served unauthenticated and only once the
  gateway and its filer are up (`default.nix:128-143`).
- `systemd.services.ix-seaweedfs` (`default.nix:145`): `ix.systemdHardening` +
  `DynamicUser = true`, `StateDirectory = seaweedfs`, and `WorkingDirectory =
  /var/lib/seaweedfs` (ProtectSystem=strict makes CWD `/` read-only, but `weed`
  resolves filer.toml/security.toml relative to CWD, so it points at the
  writable state dir).

## How it is wired

Auto-discovered as `services/seaweedfs`. Runs `pkgs.seaweedfs`; no flake output
of its own.
