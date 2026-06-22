# Minimal ix VM base image. `lib/image/oci-layer.nix` auto-enables the shared
# base profile for every image, so this module only names the publish target.
{
  ix.image = {
    name = "ix/base";
    tag = "latest";
  };
}
