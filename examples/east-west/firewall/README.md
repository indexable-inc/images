<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="allowed-client reaches service inside the east-west group; outside-client has no route">
  </picture>
</p>

# East-west firewall

How do you prove one VM can reach a service and another cannot? Put the
boundary where ix already has one: the east-west group. The HTTP service and
`allowed-client` share a group, so the client gets a private route and DNS
name; `outside-client` is left out, and its health check passes only when the
same URL fails.

## Run

```sh
# From this directory (examples/east-west/firewall in the index repo).
ix apply .#service .#allowed-client .#outside-client
```

Applying the three VMs get-or-creates the `east-west-firewall` group and
lands `service` and `allowed-client` in it; each client declares its
expected reachability as a health check. Need the repo first?
`git clone https://github.com/indexable-inc/index`.

## Verify manually

```sh
ix group members east-west-firewall
ix shell allowed-client -- curl -fsS http://service:8080/
ix shell outside-client -- curl -fsS --connect-timeout 2 http://service:8080/
```

The last command should exit non-zero. The group gives `allowed-client` a
private path to `service`; `outside-client` has no east-west route or DNS name
for that VM.

## Shape

- [`default.ix`](default.ix) wires the three VMs and places only `service`
  and `allowed-client` in the east-west group (a one-line
  `ix.networking.groups` module).
- [`service.nix`](service.nix) runs nginx on port 8080 and opens that port in
  the guest firewall.
- [`allowed-client.nix`](allowed-client.nix) checks that `http://service:8080/`
  answers over the private group.
- [`outside-client.nix`](outside-client.nix) checks that the same URL fails
  from a VM outside the group.

## Tradeoffs

ix groups are symmetric: every member can reach every other member on the
private network. For directional policy, run the listener on the service VM and
leave client-side sockets closed, or add a gateway VM that owns the stricter
rule.

The NixOS firewall still opens port 8080 on `service`. Group membership decides
which VMs have a private path to the host; nftables decides which ports are
accepted after traffic arrives.
