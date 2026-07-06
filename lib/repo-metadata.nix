# Declarative GitHub repo metadata: the About-sidebar fields (description,
# homepage, topics) for the monorepo itself, as data. `.#lib.repoMetadata`
# (lib/default.nix) renders this entry plus one entry per package mirror
# (the `mirror` attr in a package.nix, validated by packages/registry.nix)
# into the JSON that .github/workflows/repo-metadata.yml syncs to GitHub on
# every push to main; the same workflow's check job fails a PR that leaves
# any covered repo without a description or topics.
#
# One fact, one home: edit metadata here (or in a package's `mirror` attr),
# never in the GitHub UI -- the sync workflow overwrites manual edits by
# design, so a UI edit silently reverts on the next push to main.
{
  monorepo = {
    repo = "indexable-inc/index";
    description = "Open-source NixOS images, modules, and agent tooling from ix: the OSS layer over the ix VM platform.";
    homepage = "https://ix.dev";
    topics = [
      "crdt"
      "health-checks"
      "ix"
      "loro"
      "minecraft"
      "nix"
      "nixos"
      "oci-images"
      "rust"
      "svelte"
    ];
  };
}
