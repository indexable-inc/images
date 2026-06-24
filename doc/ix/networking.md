# ix networking

ix gives you two coarse networking primitives and nothing finer. East-west is private VM-to-VM reachability: VMs that share a group slug reach each other by name as `<host>.ix.internal`, and anything outside the group has no route in. North-south is public reachability: whether a VM can be reached from (or reach) the internet at all, toggled per VM. Per-port policy (allowlists, L7, WAF, rate limiting) is not an ix concern: it lives inside the image's own `networking.firewall.*`, in a sidecar, or behind a gateway VM you build (`lib/image/platform.nix:315-321`). This page covers exposing a port, making two VMs talk, and reaching a VM from your laptop. For the commands themselves see [cli.md](cli.md); for declaring this across many nodes see [fleet.md](fleet.md).

## Expose a port from an image

`ix.networking.expose.<name>` is the one declaration for "this image listens here". Each entry registers a port claim (so collisions fail at eval time), opens the in-guest firewall for the port by default, and makes the listener discoverable from sibling nodes (`lib/image/platform.nix:194-247`, `:292-313`).

```nix
ix.networking.expose.http = {
  port = 8080;
  description = "public HTTP API";
};
```

Fields (`lib/image/platform.nix:198-247`): `port`; `protocol` (`tcp` or `udp`, default `tcp`); `address` (default `"*"`, meaning every address); `namespace` (default `"default"`); `description` (defaults to the attribute name, shown in collision errors); `firewall` (default `true`). Set `firewall = false` when another mechanism already opens the port (a service's own `openFirewall = true`) and you only want the registry entry and cross-node discovery. Exposing a port does not make it public: it opens the in-guest firewall only. Reaching it from outside the VM still needs a group (east-west) or a north-south publish (below). See [services.md](services.md) for wiring this to a service module.

### The eval-time collision gotcha

Port claims are validated at build (eval) time, not at runtime. The registry tracks each listener by `(namespace, protocol, port)`, and a build fails when two claims in the same namespace collide. Address `"*"` overlaps any address, so two services that both bind `"*"` on the same protocol and port conflict even if you intended different interfaces (`lib/image/platform.nix:161-183`, `:366-375`). The error names the colliding services by their `description`, for example:

```
ix.networking.portClaims has same-namespace port collisions:
  default/tcp/8080: http (*, public HTTP API), metrics (*, prometheus)
```

To fix it, put the two services on separate fleet nodes/VMs, or choose an explicit alternate port when co-locating them in one image is intentional. The platform itself claims `tcp/5001` (ix-console) and `udp/8443` (ix-agent), so avoid those (`lib/image/platform.nix:350-364`).

## Make two VMs talk (east-west groups)

Two VMs reach each other privately when they share a group slug. Declare membership at the image level with `ix.networking.groups`, at the fleet level with `nodes.<name>.groups`, or both: the fleet plan unions them (`lib/image/platform.nix:327-345`). VMs in the same group resolve each other as `<host>.ix.internal`, where `<host>` defaults to the VM's `networking.hostName` (overridable via `ix.networking.eastWest.hostName`, `lib/image/platform.nix:322-325`). A VM outside the group has no route in.

```nix
ix.networking.groups = [ "shared-db" ];
```

Slugs are scoped per owner, limited to `[a-z0-9_-]`, and capped at 63 characters (the DNS label limit); the fleet eval rejects anything else before any RPC runs (`lib/image/platform.nix:341-343`). The platform sets `networking.search` to `ix.internal`, so unqualified names like `shared-db-primary` resolve within a group without the suffix.

To create or manage groups and membership directly from the CLI, use `ix group` (`create`, `add`, `members`, ...) and join at VM creation with `ix new --group <slug>` (repeatable). Run `ix group --help` and `ix new --help` for the exact flags.

## Make a VM public (north-south)

Public reachability is per VM and is a plain on/off, set at creation or changed later:

- `ix new --ipv4` allocates a public IPv4 address (default is IPv6/proxy-based ingress).
- `ix new --l7-proxy-port <PORT>` publishes an application port through the HTTP/TLS proxy; repeat for several ports.
- `ix share <vm> <port>` publishes a guest TCP port on a public share hostname. Use `--public` for an open hostname or `--to <email>` (repeatable) for an email-gated one. Bare `ix share` lists existing shares.
- `ix vm set --internet-ingress on|off` and `--internet-egress on|off` toggle inbound and outbound internet for an existing VM. East-west group traffic and the control plane keep working regardless.

There is deliberately no per-port grammar on these toggles: finer filtering belongs in the image's `networking.firewall.*`. Run `ix vm set --help`, `ix new --help`, and `ix share --help` for the full flag set. See [lifecycle.md](lifecycle.md) for when these take effect across create/replace/switch.

## Reach a VM from your laptop

Two ways, depending on whether you want the whole group or one port:

- `ix net up <group>` joins a group's private overlay from your machine (Linux only). It creates a TUN device, routes the group's private address range through it, and serves `<host>.ix.internal` from a local DNS stub, so every member VM is reachable by name on any port with nothing public. It runs in the foreground (Ctrl-C disconnects) and requires `CAP_NET_ADMIN` (run under sudo) and systemd-resolved.
- `ix port-forward <vm> <local:remote>` opens a private debug tunnel from a local port to a port inside the VM (for example `ix port-forward web 8080:80`). This is for private debugging only; it does not publish the service as public ingress.

Run `ix net --help` and `ix port-forward --help` for flags.
