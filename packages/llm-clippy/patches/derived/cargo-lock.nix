# Derived patch (registered in lib/fork-packages.nix `derivedPatches`): track
# a Cargo.lock in clippy's tree. Upstream deliberately gitignores the
# lockfile; our nix consumers need a stable one next to the source
# (packages/llm-clippy reads `cargoLock.lockFile` from the patched tree), so
# this patch un-ignores `/Cargo.lock` and copies in the committed lockfile
# that lives next to this generator (./Cargo.lock).
#
# Refresh policy: the lockfile moves ONLY with the pinned nightly / clippy-src
# bump, exactly as the stored-diff predecessor did. When a fork rebase
# moves the clippy base, regenerate ./Cargo.lock with the pinned toolchain
# (`cargo generate-lockfile` in the patched tree) and commit it here in the
# same change.
{
  pkgs,
  src,
  ...
}:
pkgs.runCommand "llm-clippy-cargo-lock.patch" {} ''
  cp -r ${src} old
  cp -r ${src} new
  chmod -R u+w old new

  # Structural guard: upstream starting to track its own lockfile means this
  # generator (and the committed copy) must be retired, not silently fought.
  if [ -e old/Cargo.lock ]; then
    echo 'cargo-lock generator: upstream tree now tracks Cargo.lock; retire this generator' >&2
    exit 1
  fi

  # The un-ignore line lands directly under the `*Cargo.lock` ignore rule it
  # carves the exception out of. Guard the anchor structurally: exactly one
  # such rule, and no pre-existing exception.
  anchors=$(grep -cx '\*Cargo\.lock' new/.gitignore || true)
  if [ "$anchors" -ne 1 ]; then
    echo "cargo-lock generator: expected exactly one '*Cargo.lock' ignore rule in .gitignore, found $anchors" >&2
    exit 1
  fi
  if grep -qx '!/Cargo.lock' new/.gitignore; then
    echo 'cargo-lock generator: .gitignore already un-ignores /Cargo.lock' >&2
    exit 1
  fi
  sed -i '/^\*Cargo\.lock$/a !/Cargo.lock' new/.gitignore

  cp ${./Cargo.lock} new/Cargo.lock
  # Structural guards on the committed lockfile: the right format and the
  # workspace's own crate resolved (a wrong or truncated file fails here, not
  # deep inside a consumer build).
  if ! grep -qx 'version = 4' new/Cargo.lock; then
    echo 'cargo-lock generator: committed Cargo.lock is not lockfile format v4' >&2
    exit 1
  fi
  if ! grep -qx 'name = "clippy"' new/Cargo.lock; then
    echo 'cargo-lock generator: committed Cargo.lock does not resolve the clippy crate itself' >&2
    exit 1
  fi
  packages=$(grep -cx '\[\[package\]\]' new/Cargo.lock)
  if [ "$packages" -eq 0 ]; then
    echo 'cargo-lock generator: committed Cargo.lock resolves no packages' >&2
    exit 1
  fi
  echo "cargo-lock generator: tracked Cargo.lock with $packages resolved packages"

  # The diff headers embed file mtimes; pin them so the patch bytes are
  # reproducible across builds.
  find old new -exec touch -h -d @0 {} +
  status=0
  diff -ruN old new > "$out" || status=$?
  if [ "$status" -ne 1 ]; then
    echo "cargo-lock generator: expected a non-empty diff, got diff exit $status" >&2
    exit 1
  fi
''
