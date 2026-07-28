# Images

An ix VM boots from one image. You build that image from a NixOS configuration
with the index library, publish it to your private registry namespace, then
point a VM at it. This page is the build -> tag -> publish -> boot model and the
`ix image` verbs; for flags on any verb run `ix image --help`.

## The model

1. **Build.** Evaluate a NixOS config through the index image library
   (`index.lib.mkImage`, `lib/image/default.nix:184`). A baseline platform
   module is applied to every image automatically (`./platform.nix`, layered in
   at `lib/image/default.nix:68-90`). Each image is self-contained: ix runs one
   image per VM, it does not stack images at runtime.
2. **Tag.** The image name comes from `ix.image.name`
   (`lib/image/oci-layer.nix:17`). In a fleet the node name seeds
   `ix.image.name` by default (`lib/image/fleet.nix:304`). The registry tag is
   chosen at publish time by the destination ref.
3. **Publish.** The bootable artifact is a CAS manifest, and it is built on the
   ix side rather than here: index declares the contract as
   `ix.build.casImageBuilder` and ix supplies the implementation
   (`lib/image/cas-layer.nix:34`, whose doc-comment is canonical). Building
   `config.ix.build.casImage` yields a directory holding `manifest.cas` and
   `locator.bin`, and `ix image push-manifest` uploads that pair:

   ```sh
   ix image push-manifest --region <region> \
     --locator <out>/locator.bin <out>/manifest.cas <destination>
   ```

   A bare destination is stored under `registry.ix.dev/<your-username>/`.
4. **Boot.** Create a VM from a registry ref with `ix apply <ref>` (or
   `ix run`). See [cli.md](cli.md).

`mkImage` returns `config.ix.build.ociImage`, an OCI archive
(`lib/image/oci-layer.nix:58`). Nothing boots from it. ix deleted the OCI
ingest pipeline in ENG-6044 phase 7 (ix#6930), which removed `ix image push`
and every `docker://` source with it, leaving `push-manifest` as the only push
path; the archive and its builder survive only until the companion index change
retires `lib/image/oci-layer.nix`.

## Publishing, listing, removing

`ix image` manages the registry layer before any VM exists:

- `ix image push-manifest --locator <locator.bin> <manifest.cas> <destination>` -
  upload a CAS manifest. Chunk bytes stream straight from their origin
  `/nix/store` files, which is what the locator names, so the store paths the
  manifest was built from have to be present on the pushing machine. `--public`
  lets other ix users pull it; `--region` selects the target registry. This is
  builder-facing: the publish pipeline is its usual caller, and `--admin`
  unlocks the system namespace.
- `ix image ls` - list system images and your private images in a region.
- `ix image rm <reference>` - delete one tag you own (digest refs are
  rejected).

## Base images

`ix apply ix/base:latest` boots the stock NixOS system image; on flake targets,
`ix apply --base` also defaults to `ix/base:latest`.
Use it for a general Linux VM, or
pass your own fully-qualified registry ref to boot an application image.
The flake-converge path needs a NixOS base so it can activate closures in place.

## Example

```nix
# image.nix - a NixOS module evaluated by index.lib.mkImage
{
  ix.image.name = "hello";
  # ... your services, packages, etc.
}
```

```sh
# build the CAS manifest for that config (the ix flake owns the builder), then:
ix image push-manifest --region us-west-1 \
  --locator ./result/locator.bin ./result/manifest.cas hello:v1
ix apply registry.ix.dev/<you>/hello:v1 --name hello
```

## Images in a fleet

A fleet node carries two images (`lib/image/fleet.nix:291-300`):

- **`bootstrapImage`** - the create-time image used to first materialize a
  missing node. Defaults to the shared NixOS bootstrap image under
  `registry.ix.dev/...` (`lib/image/fleet.nix:48-49`,
  `lib/image/default.nix:104-107`).
- **`replacementImage`** (`{ imageName, destination, sourceInstallable }`) -
  the CAS-manifest image `up`/`replace` build (via the `.#<node>` flake attr)
  and push from your config (`lib/image/fleet.nix:311-322`). `destination`
  defaults to `<imageName>:latest` (`lib/image/fleet.nix:269`).

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

- [cli.md](cli.md) - `ix apply` and the VM verbs.
- [lifecycle.md](lifecycle.md) - when an image swap recreates the VM.
- [services.md](services.md) - the ready-made service modules you compose into an image.
- [fleet.md](fleet.md) - multi-VM plans, bootstrap vs replacement images.
- [secrets.md](secrets.md) - attaching secrets to a VM at boot.
