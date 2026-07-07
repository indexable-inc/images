# Mark a derivation as substitutable even though its nixpkgs builder opts out.
#
# The nixpkgs trivial builders (`writeText`, `writeTextFile`, `linkFarm`,
# `applyPatches`, ...) hardcode `preferLocalBuild = true; allowSubstitutes =
# false`: rebuilding a tiny text file or symlink farm locally is cheaper than a
# narinfo round-trip, so on the platform that produced it substitution is a net
# loss. That default is wrong for any such derivation that lands in the darwin
# cross lane's eval-time IFD closure (the cargo-unit `vendorDir` / `unitGraphJson`
# / `unitsNix` roots, and the de-forked `patchedSrcFor` sources they pull in):
#
#   - The derivation is `x86_64-linux`, so an aarch64-darwin consumer cannot
#     build it. It is forced at *eval time* (`import unitsNix` reaches the whole
#     vendor + patched-source closure), so a build is not even an option.
#   - With `allowSubstitutes = false`, Nix never creates a substitution goal
#     (`derivation-goal.cc` gates it on `substitutesAllowed`, which returns the
#     derivation's `allowSubstitutes` unless `always-allow-substitutes` is set).
#     It goes straight to build, fails, and eval dies with `platform mismatch`
#     no matter how healthy the cache is -- the pushed, signed output is simply
#     never consulted.
#
# So on these nodes the trade-off inverts: a Mac can only obtain the output by
# substitution, and forcing `allowSubstitutes = true` is what makes the pushed
# cache output usable. First hit and fixed for the `linkFarm` vendor dir alone
# (#1711); this is the shared owner of that fact for every trivial-builder node
# in the closure (#6318).
#
# drv -> drv, substitutable regardless of the builder's opt-out.
drv:
drv.overrideAttrs (_: {
  allowSubstitutes = true;
})
