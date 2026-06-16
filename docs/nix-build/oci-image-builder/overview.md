# oci-image-builder

`packages/oci-image-builder` turns a `dockerTools.streamLayeredImage` layer plan
into an OCI image. Beyond the legacy one-shot (plan -> tar) it splits the work
into a tiny content-addressed description (`image.json`) and a separate
materialization step, so an image can be described cheaply and its bytes built
only when needed. It is a Rust workspace crate; the layer model it serves is the
[nix-lib](../../nix-lib/common.md) `lib/image` tree.

## Purpose

`streamLayeredImage` produces a `conf.json` layer plan; turning it into an OCI tar
is expensive (tens of MiB and up). The description records each layer's digest and
how to regenerate it, not its bytes, so it is a few KiB: cheap to build and cache,
and the same description can later target a registry push that uploads only
missing layers, or a rootfs image, without rebuilding (`README.md:29-37`).

## Modes (`src/main.rs`)

The `Mode` enum (`src/main.rs:36`) plus two sharded subcommands dispatched before
the positional parser (`src/main.rs:189`):

| invocation | input -> output |
| --- | --- |
| `oci-image-builder <conf.json> <out.tar>` | legacy one-shot: plan -> OCI tar (`Build`). Default, so the NixOS image path is unchanged. |
| `oci-image-builder describe <conf.json> <image.json>` | plan -> description, no layer bytes (`Describe`). |
| `oci-image-builder materialize <image.json> <out.tar>` | description -> OCI tar, regenerating and verifying bytes (`Materialize`). |
| `oci-image-builder base-desc <base-archive.tar> <base.json>` | describe the immutable base image's layers (`run_base_desc`, `src/main.rs:267`). |
| `oci-image-builder layer-desc --uid N --gid N --mtime T <layer.json> <store-path>...` | describe one store layer as its own derivation (`run_layer_desc`, `src/main.rs:218`). |
| `oci-image-builder assemble-desc --base <base.json> <conf.json> <image.json> <layer.json>...` | stitch precomputed layer descriptions into one `image.json`, re-tarring nothing (`run_assemble_desc`, `src/main.rs:308`). |

`<conf.json>` is the `passthru.conf` `streamLayeredImage` produces (`Config`,
`src/main.rs:62`). The legacy one-shot is `describe` then `materialize` in one
pass (`README.md:20-22`).

## Per-layer sharding

`describe` hashes every layer in one process, so changing one store path re-tars
the whole closure. The sharded path (`base-desc` / `layer-desc` /
`assemble-desc`) makes each layer its own content-addressed derivation, driven by
an IFD that reads the layer partition out of `conf.json` (`README.md:39-61`):

```
conf.json --(IFD)--> Nix
   base archive  --> base-desc     --> base.json   (cached; base is pinned + immutable)
   store_layers[k]--> layer-desc   --> layerK.json (one derivation each; same paths = same drv)
   base.json + layerK.json + conf --> assemble-desc --> image.json (pure stitch, no bytes)
```

`assemble-desc` reads the precomputed digests, computes the customisation layer's
digest from its prebuilt checksum, and merges the config, so editing one store
path re-tars only that layer; the rest and the base are cache hits.

## image.json schema (`Description`, `src/main.rs:89`)

A `Description` records `schema_version`, architecture, `created`/`mtime`/
`uid`/`gid`/`store_dir`, the merged OCI `config`, and a `layers` list. Each
`LayerDesc` (`src/main.rs:105`) carries `digest`/`diff_id`/`size` plus a flattened
`LayerSource` (`src/main.rs:116`) that selects how `materialize` regenerates the
bytes:

- `store`: re-tar the listed Nix store paths (deterministic from the paths).
- `base`: copy the named member out of the base docker-archive.
- `customisation`: copy the prebuilt `layer.tar` from its derivation output.

For uncompressed tar layers the blob digest equals the diff id, which is why a
pulled base layer (skopeo writes them uncompressed) round-trips without a separate
compressed digest (`README.md:95-97`, schema in `README.md:65-93`).

## Verification and efficiency policy

- **Materialize verifies bytes.** Each layer is regenerated deterministically and
  checked against the recorded digest, so a description that no longer reproduces
  its bytes fails the build instead of shipping a wrong image
  (`README.md:33-36`).
- **Efficiency policy** (`EfficiencyPolicy`, `src/main.rs:43`): the layer set is
  analyzed for paths repeated across layers (wasted bytes). Flags
  `--min-efficiency`, `--max-wasted-bytes`, `--max-wasted-percent`,
  `--efficiency-top-paths`, `--skip-efficiency-check` apply to `build`,
  `describe`, and `materialize` (`README.md:24-27`). Defaults: min efficiency
  0.95, max wasted 20 MiB, max wasted 20%, top 10 paths (`src/main.rs:14-17`).
  Base layers from a `fromImage` are excluded (pulled and immutable). Because the
  cross-layer analysis needs layer bytes the sharded path does not keep, the
  policy is enforced at `materialize` time where the bytes already exist
  (`README.md:59-61`).

## Build and packaging

`default.nix` selects the binary via `ix.cargoUnit.selectBinaryWithTests`. It is
`inRustWorkspace`, `flake = true`, `packageSet = true`, and also an `overlay`
entry built with `buildIxRustTool` (`package.nix:7-17`). Flake output /
main program: `oci-image-builder`. Deps (`Cargo.toml`): `tar`, `sha2`, `hex`,
`chrono`, `serde`/`serde_json`, `tempfile`. See `bench.sh` for the cold/warm
benchmark.
