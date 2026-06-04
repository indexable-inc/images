# oci-image-builder

Turns a `streamLayeredImage` layer plan into an OCI image. It runs in three
modes around a content-addressed description, `image.json`, so an image can be
described cheaply and materialized into bytes only when needed (see #679).

## Modes

```
oci-image-builder            <conf.json> <out.tar>   # legacy one-shot: plan -> OCI tar
oci-image-builder describe   <conf.json> <image.json> # plan -> description, no layer bytes
oci-image-builder materialize <image.json> <out.tar>  # description -> OCI tar
```

`<conf.json>` is the `passthru.conf` that `dockerTools.streamLayeredImage`
produces. The legacy mode is `describe` followed by `materialize` in one pass and
stays the default so the NixOS image path is unchanged.

The efficiency flags (`--min-efficiency`, `--max-wasted-bytes`,
`--max-wasted-percent`, `--efficiency-top-paths`, `--skip-efficiency-check`)
apply to `describe` and the legacy build. Base layers from a `fromImage` are
excluded from the analysis: they are pulled and immutable.

## Why describe vs materialize

The description records each layer's digest and how to regenerate it, not its
bytes. It is tiny (a few KiB) where the tar is the full image (tens of MiB and
up), so it is cheap to build and cache. Materialize regenerates each layer's
bytes deterministically and verifies them against the recorded digest, so a
description that no longer reproduces its bytes fails the build instead of
shipping a wrong image. The same description can later target a registry push
that uploads only missing layers, or a rootfs image, without rebuilding.

## image.json schema

```jsonc
{
  "schema_version": 1,
  "architecture": "amd64",
  "created": "1970-01-01T00:00:01Z",
  "mtime": "1",                       // unix seconds, as a string
  "uid": "0",
  "gid": "0",
  "store_dir": "/nix/store",
  "config": { "Cmd": ["/bin/sh"] },   // OCI image config, base Env merged under final
  "layers": [
    // bottom of the stack first
    { "digest": "sha256:...", "diff_id": "sha256:...", "size": 1234,
      "kind": "base", "archive": "/nix/store/...-docker-image.tar", "member": "abc.tar" },
    { "digest": "sha256:...", "diff_id": "sha256:...", "size": 5678,
      "kind": "store", "paths": ["/nix/store/...-glibc-2.42"] },
    { "digest": "sha256:...", "diff_id": "sha256:...", "size": 90,
      "kind": "customisation", "dir": "/nix/store/...-customisation-layer" }
  ]
}
```

Layer `kind` selects how `materialize` regenerates the bytes:

- `store`: re-tar the listed Nix store paths (deterministic from the paths).
- `base`: copy the named member out of the base docker-archive.
- `customisation`: copy the prebuilt `layer.tar` from its derivation output.

For uncompressed tar layers the blob digest equals the diff id, which is why a
pulled base layer (skopeo writes them uncompressed) round-trips without a
separate compressed digest.
