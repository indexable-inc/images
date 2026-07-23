<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="client joins the group via the fleet spec, api via its image; both union into one east-west network">
  </picture>
</p>

# Declared groups

Should a VM's group membership live in the image or in the deployment
wiring? Both work, and this two-VM example shows each source. The `api`
image sets `ix.networking.groups = [ "declared-groups" ]` in its own module,
so any deployment of that image joins the deployer's `declared-groups`
network; the `client`'s image is group-agnostic, and its membership is a
one-line module added in [`default.ix`](default.ix). Both sources land in
the same network.

## Run

```sh
# From this directory (examples/declared/groups in the index repo).
ix apply .#api .#client
```

Applying the two VMs get-or-creates the `declared-groups` group under your
account and adds both; the client's health check curls the api over the
private group network. Need the repo first?
`git clone https://github.com/indexable-inc/index`.

## Verify manually

```sh
ix group members declared-groups
ix shell client -- curl -fsS http://api:8080/
```

## Shape

- [`default.ix`](default.ix) wires the two VMs; note `api` gains no group
  at the wiring layer, while `client` gets its membership from a one-line
  module there.
- [`api.nix`](api.nix) carries the group in the image
  (`ix.networking.groups`) and exposes port 8080 to group members.
- [`client.nix`](client.nix) checks that `http://api:8080/` answers over
  the private network.

## Where the slugs live

Group slugs are scoped to the deploying user (`UNIQUE (owner_id, slug)` on
the server), so a common name like `declared-groups` in a published image
never collides with another user's group of the same name. Slugs are
`[a-z0-9_-]`, max 63 chars (the DNS label limit); the eval rejects anything
else before any RPC runs.
