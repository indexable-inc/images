# Images

An ix VM boots from one OCI image. You build that image from a NixOS
configuration with the index library, push it to your private registry
namespace, then point a VM at it. This page is the build -> tag -> push -> boot
model and the `ix image` verbs; for flags on any verb run `ix image --help`.

## The model

1. **Build.** Evaluate a NixOS config through the index image library
   (`index.lib.mkImage`, `lib/image/default.nix:99`). A baseline platform
   module is applied to every image automatically (`./platform.nix`, layered in
   at `lib/image/default.nix:68-90`). The evaluated config yields the OCI
   archive at `config.ix.build.ociImage`
   (`lib/image/oci-layer.nix:28-31`, `:64`); `mkImage` returns exactly that
   derivation. Each image is self-contained: ix runs one image per VM, it does
   not stack images at runtime (`lib/image/default.nix:92-99`).
2. **Tag.** The image name and tag come from `ix.image.name` and
   `ix.image.tag` (`lib/image/oci-layer.nix:18-26`; tag defaults to `latest`).
   In a fleet the node name seeds `ix.image.name` by default
   (`lib/image/fleet.nix:218`). See `examples/nginx-lifecycle/default.nix:4`
   for a real `ix.image.tag = "nginx-lifecycle";`.
3. **Push.** Send the archive to your registry namespace with
   `ix image push <source> <destination>`. A bare destination is stored under
   `registry.ix.dev/<your-username>/`.
4. **Boot.** Create a VM from a registry ref with `ix new <ref>` (or
   `ix run`). See [cli.md](cli.md).

## Pushing, listing, removing

`ix image` manages the registry layer before any VM exists:

- `ix image push <source> <destination>` - push an archive or ref. A plain
  path source is read as `oci-archive:<path>`; a plain ref as `docker://<ref>`.
  `--public` lets other ix users pull it; `--region` selects the target
  registry.
- `ix image ls` - list system images and your private images in a region.
- `ix image rm <reference>` - delete one tag you own (digest refs are
  rejected).

## Base images

`ix new` and `ix up --base` default to `ix/base:latest`, a NixOS system image.
Use it for a general Linux VM, or
pass your own fully-qualified registry ref to boot an application image.
`ix up` needs a NixOS base so it can activate closures in place.

## Example

```nix
# image.nix - a NixOS module evaluated by index.lib.mkImage
{
  ix.image = {
    name = "hello";
    tag = "v1";
  };
  # ... your services, packages, etc.
}
```

```sh
# build the archive (your flake exposes the mkImage output), then:
ix image push ./result registry.ix.dev/<you>/hello:v1
ix new registry.ix.dev/<you>/hello:v1 --name hello
```

## Images in a fleet

A fleet node carries two images (`lib/image/fleet.nix:291-300`):

- **`bootstrapImage`** - the create-time image used to first materialize a
  missing node. Defaults to the shared NixOS bootstrap image under
  `registry.ix.dev/...` (`lib/image/fleet.nix:48-49`,
  `lib/image/default.nix:104-107`).
- **`replacementImage`** (`{ imageName, imageTag, destination, source,
  sourceDrv }`) - the image `up`/`replace` build and push from your config
  (`lib/image/fleet.nix:292-300`). `destination` defaults to
  `<imageName>:<imageTag>` (`lib/image/fleet.nix:248`).

See [fleet.md](fleet.md) for the authoring surface.

## Swapping a VM's image recreates it

Each VM boots one image, and image swap is delete-then-create, not in-place:
`client.create` inserts against a `UNIQUE (owner, name)` constraint, so
changing a node's image removes and recreates it
(`doc/ix-fleet/overview.md:107-109`). In a fleet, `replace` always does this,
and `deployment.recreateOnUp = true` makes `up` do it too (see
`examples/nginx-lifecycle/default.nix:7`). For when this recreate happens across
create/replace/switch, see [lifecycle.md](lifecycle.md); the full lifecycle
reference is [../ix-fleet/overview.md](../ix-fleet/overview.md).

## See also

- [cli.md](cli.md) - `ix new`, `ix up`, and the VM verbs.
- [lifecycle.md](lifecycle.md) - when an image swap recreates the VM.
- [services.md](services.md) - the ready-made service modules you compose into an image.
- [fleet.md](fleet.md) - multi-VM plans, bootstrap vs replacement images.
- [secrets.md](secrets.md) - attaching secrets to a VM at boot.
