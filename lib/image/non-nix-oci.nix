# Builder for OCI images on a non-NixOS base (ubuntu, debian, distroless).
#
# Unlike `mkImage`, the result is not a NixOS system: there is no systemd PID 1,
# no `/init`, no closure-as-rootfs. The pinned base image's userland is the
# rootfs, and `contents` are added as extra Nix store layers on top. This is the
# "agent lands in a normal Linux userland" path.
#
# The base is pulled by digest (`dockerTools.pullImage`), so the build stays
# reproducible and pulls no floating tag at build time. `streamLayeredImage`
# plans the layers and `oci-image-builder` finalizes the OCI archive, the same
# tool the NixOS path uses, now with `fromImage` support. ix still receives a
# standard OCI archive, so nothing downstream of `ix image push` changes.
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
    in
    pkgs.runCommand "${name}-oci.tar"
      {
        nativeBuildInputs = [ pkgs.oci-image-builder ];
        passthru = { inherit stream baseImage; };
      }
      ''
        oci-image-builder ${lib.escapeShellArgs (efficiencyArgs ++ [ "${stream.passthru.conf}" ])} "$out"
      '';
}
