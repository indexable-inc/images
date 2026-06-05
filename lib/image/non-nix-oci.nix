# Builder for OCI images on a non-NixOS base (ubuntu, debian, distroless).
#
# Unlike `mkImage`, the result is not a NixOS system: there is no systemd PID 1,
# no `/init`, no closure-as-rootfs. The pinned base image's userland is the
# rootfs, and `contents` are added as extra Nix store layers on top. This is the
# "agent lands in a normal Linux userland" path.
#
# The base is pulled by digest (`dockerTools.pullImage`), so the build stays
# reproducible and pulls no floating tag at build time. `streamLayeredImage`
# plans the layers, then the build runs in two stages around a content-addressed
# description (see #679): `oci-image-builder describe` emits a tiny `image.json`
# recording each layer's digest and how to regenerate it, and `materialize`
# regenerates the OCI tar from that description, verifying every layer against
# its recorded digest. The description is the durable artifact; the tar is
# reproduced on demand, so ix still receives a standard OCI archive and nothing
# downstream of `ix image push` changes. The description is exposed on the
# result's `passthru.description`.
{
  lib,
  pkgs,
}:
let
  defaultEfficiency = {
    enable = true;
    minEfficiency = 0.95;
    maxWastedBytes = 20 * 1024 * 1024;
    maxWastedPercent = 0.20;
    reportTopPaths = 10;
  };
in
{
  /**
    Build one OCI archive from a pinned non-Nix base image plus Nix packages.

    Arguments:
    - `name`/`tag`: OCI repository and tag.
    - `baseImage`: a derivation producing a docker-archive tarball, typically
      `pkgs.dockerTools.pullImage { imageName; imageDigest; sha256; }`. Pin by
      digest so the build is reproducible.
    - `contents`: Nix store packages layered on top of the base userland.
    - `config`: OCI config (`Entrypoint`, `Cmd`, `Env`, `WorkingDir`, ...). The
      base image `Env` is merged underneath by the builder; these entries win.
    - `maxLayers`: layer budget left under the 127-layer registry cap after the
      base layers.
    - `efficiency`: layer-efficiency policy, mirrored from
      `ix.build.ociEfficiency`. Base layers are excluded from the analysis: they
      are pulled and immutable, so their internal duplication is not ours to fix.

    Returns the OCI-archive derivation; pass it to `ix image push` or expose it
    as a `packages.<system>.<name>` output.
  */
  mkNonNixImage =
    {
      name,
      baseImage,
      tag ? "latest",
      contents ? [ ],
      config ? { },
      maxLayers ? 64,
      efficiency ? { },
    }:
    let
      policy = defaultEfficiency // efficiency;

      stream = pkgs.dockerTools.streamLayeredImage {
        inherit
          name
          tag
          maxLayers
          contents
          config
          ;
        fromImage = baseImage;
      };

      efficiencyArgs =
        if policy.enable then
          [
            "--min-efficiency"
            (toString policy.minEfficiency)
            "--max-wasted-bytes"
            (toString policy.maxWastedBytes)
            "--max-wasted-percent"
            (toString policy.maxWastedPercent)
            "--efficiency-top-paths"
            (toString policy.reportTopPaths)
          ]
        else
          [ "--skip-efficiency-check" ];

      # The layer partition is a build output of `streamLayeredImage`, so import
      # `conf.json` once (IFD) to learn which store paths land in which layer,
      # then describe each layer in its own derivation. Editing one store path
      # re-tars only that layer; the rest are derivation cache hits, and a layer
      # with the same paths is the same derivation across images and rebuilds.
      # `readFile` of an IFD output carries the conf's store-path context, which
      # `fromJSON` rejects, so discard it here and re-attach per path below.
      plan = builtins.fromJSON (
        builtins.unsafeDiscardStringContext (builtins.readFile "${stream.passthru.conf}")
      );

      # The parse above strips store context, so re-attach it per path. This makes
      # each layer derivation depend on exactly its own paths (not the whole
      # closure via `conf.json`), which is what keeps a one-path change from
      # invalidating the other layers.
      withStoreContext =
        path:
        builtins.appendContext path {
          ${path} = {
            path = true;
          };
        };

      # The base image description: layer digests plus the base container config,
      # built once from the digest-pinned, immutable base archive. It depends on
      # nothing in the closure, so it never reruns when `contents` change and is
      # shared by every image on the same base.
      baseDesc = pkgs.runCommand "oci-base-desc.json" {
        nativeBuildInputs = [ pkgs.oci-image-builder ];
      } ''oci-image-builder base-desc ${baseImage} "$out"'';

      # One description per store layer. The derivation name is image-independent
      # so a layer with the same paths, uid, gid, and mtime is the same derivation
      # across every image: built once, shared in the store and binary cache.
      storeLayerDescs = map (
        paths:
        pkgs.runCommand "oci-store-layer.json" { nativeBuildInputs = [ pkgs.oci-image-builder ]; } ''
          oci-image-builder layer-desc \
            --uid ${lib.escapeShellArg (toString plan.uid)} \
            --gid ${lib.escapeShellArg (toString plan.gid)} \
            --mtime ${lib.escapeShellArg (toString plan.mtime)} \
            "$out" ${lib.escapeShellArgs (map withStoreContext paths)}
        ''
      ) plan.store_layers;

      # The durable artifact: a tiny content-addressed `image.json` stitched from
      # the cached base and per-layer descriptions. Nothing is re-tarred here, so
      # with the parts cached this is pure JSON assembly: a one-layer change costs
      # one layer re-tar plus this near-instant stitch.
      description =
        pkgs.runCommand "${name}-image.json"
          {
            nativeBuildInputs = [ pkgs.oci-image-builder ];
            passthru = {
              inherit
                stream
                baseImage
                baseDesc
                storeLayerDescs
                ;
            };
          }
          ''
            oci-image-builder assemble-desc \
              --base ${baseDesc} \
              ${stream.passthru.conf} "$out" \
              ${lib.escapeShellArgs (map toString storeLayerDescs)}
          '';
    in
    # The OCI tar, regenerated from the description on demand. Kept as the default
    # output so `ix image push` is unchanged; the description is the input, so the
    # build graph is describe -> materialize. The efficiency policy is enforced
    # here at materialize time, where the regenerated bytes already exist, since
    # the sharded describe path cannot run the cross-layer analysis. The small
    # description is reachable via `passthru` for callers that want it directly.
    pkgs.runCommand "${name}-oci.tar"
      {
        nativeBuildInputs = [ pkgs.oci-image-builder ];
        passthru = {
          inherit
            stream
            baseImage
            description
            baseDesc
            storeLayerDescs
            ;
        };
      }
      ''
        oci-image-builder materialize ${lib.escapeShellArgs efficiencyArgs} ${description} "$out"
      '';
}
