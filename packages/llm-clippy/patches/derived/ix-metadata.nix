# Derived patch (registered in lib/fork-packages.nix `derivedPatches`): stamp
# the `[package.metadata.ix.inputs]` stanza into every `[package]` manifest of
# clippy's tree so cargo-unit can treat the fork's crates like repo crates.
#
# The patch is a derivation, not a stored diff: it regenerates against the
# tree it patches (the pinned `clippy-src` with the earlier series applied, so
# fixture manifests the lint patches create are stamped too), a base bump can
# never conflict on it, and a manifest upstream adds tomorrow is stamped
# automatically (the stored-diff predecessor missed exactly one such late
# manifest, tests/ui-cargo/module_style/inline_mod).
#
# `build = ["."]` iff the crate ships a build script: cargo-unit needs the
# build-script inputs only where a build.rs exists next to the manifest.
{
  pkgs,
  src,
  ...
}:
pkgs.runCommand "llm-clippy-ix-metadata.patch" {} ''
  cp -r ${src} old
  cp -r ${src} new
  chmod -R u+w old new

  # Structural guard: if upstream ever grows its own ix stanza, appending a
  # second one would corrupt the manifests. Fail loudly instead.
  if grep -rq --include=Cargo.toml 'package\.metadata\.ix\.inputs' old; then
    echo 'ix-metadata generator: upstream tree already carries package.metadata.ix.inputs; retire or rework this generator' >&2
    exit 1
  fi

  count=0
  while IFS= read -r manifest; do
    dir=$(dirname "$manifest")
    build='[]'
    if [ -f "$dir/build.rs" ]; then
      build='["."]'
    fi
    # One upstream fixture manifest ends without a newline; appending the
    # stanza must not glue onto its last line.
    if [ -n "$(tail -c1 "$manifest")" ]; then
      echo >> "$manifest"
    fi
    printf '[package.metadata.ix.inputs]\ncompile = ["."]\nbuild = %s\n' "$build" >> "$manifest"
    count=$((count + 1))
  done < <(grep -rl --include=Cargo.toml -x '\[package\]' new | sort)

  # Structural guard, no magic totals: every `[package]` manifest must have
  # received exactly one stanza, and there must be at least one. A zero or
  # partial count means the tree changed shape under us; never no-op silently.
  stamped=$(grep -rl --include=Cargo.toml -x '\[package.metadata.ix.inputs\]' new | wc -l)
  if [ "$count" -eq 0 ] || [ "$stamped" -ne "$count" ]; then
    echo "ix-metadata generator: stamped $stamped of $count [package] manifests (need every one, and at least one)" >&2
    exit 1
  fi
  echo "ix-metadata generator: stamped $count Cargo manifests"

  # The diff headers embed file mtimes; pin them so the patch bytes are
  # reproducible across builds.
  find old new -exec touch -h -d @0 {} +
  status=0
  diff -ruN old new > "$out" || status=$?
  # diff exits 1 iff it found a delta; 0 (silent no-op) and 2 (trouble) both
  # mean the generator did not do its job.
  if [ "$status" -ne 1 ]; then
    echo "ix-metadata generator: expected a non-empty diff, got diff exit $status" >&2
    exit 1
  fi
''
