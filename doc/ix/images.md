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
2. **Tag.** The image name comes from `ix.image.name`
   (`lib/image/oci-layer.nix:17-20`); the built archive is always tagged
   `latest` (`lib/image/oci-layer.nix:79`). In a fleet the node name seeds
   `ix.image.name` by default (`lib/image/fleet.nix:238`). The registry tag is
   chosen at push time by the destination ref.
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
  ix.image.name = "hello";
  # ... your services, packages, etc.
}
```

```sh
# build the archive (your flake exposes the mkImage output), then:
ix image push ./result registry.ix.dev/<you>/hello:v1
ix new registry.ix.dev/<you>/hello:v1 --name hello
```

## Images in a fleet

A fleet node's `bootstrapImage` is the create-time image used to materialize a
missing VM. It defaults to the shared NixOS bootstrap image under
`registry.ix.dev/...`. After creation, `ix up` builds and activates the node's
`nixosConfigurations.<name>` directly on that VM.

See [fleet.md](fleet.md) for the authoring surface.

## See also

- [cli.md](cli.md) - `ix new`, `ix up`, and the VM verbs.
- [lifecycle.md](lifecycle.md) - when an image swap recreates the VM.
- [services.md](services.md) - the ready-made service modules you compose into an image.
- [fleet.md](fleet.md) - multi-VM plans and bootstrap images.
- [secrets.md](secrets.md) - attaching secrets to a VM at boot.
