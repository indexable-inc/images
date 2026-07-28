# Usage-telemetry wrapper seam (index#3802).
#
# `withUsage pkg { ... }` rewraps every executable in `pkg`'s bin/ with
# `ix-wrap` so invocations and failures land in the local usage store
# (spool + SQLite; see packages/usage). Policy is rendered per binary into a
# JSON spec consumed via IX_USAGE_SPEC, the same wiring shape as
# config-launch's IX_LAUNCH_SPEC. The wrapper records after the child exits,
# so the tool's own latency is untouched; `mode = "count-only"` records then
# exec()s for hot-path tools called in tight loops.
{
  lib,
  pkgs,
  ix,
}: pkg: {
  # Package id recorded in counts; defaults to the wrapped package's name.
  id ? lib.getName pkg,
  version ? pkg.version or "unknown",
  # "observe" (default) or "count-only".
  mode ? "observe",
  # Whether failing invocations keep argv/cwd in the LOCAL database (never
  # uploaded either way).
  errors ? true,
  # Whether invocations may kick the detached `ix-usage upload --if-due`.
  uploader ? true,
}: let
  ixWrap = ix.rustWorkspace.units.binaries.ix-wrap;
  ixUsage = ix.rustWorkspace.units.binaries.ix-usage;
  specBase =
    {
      pkg = id;
      inherit version mode errors;
    }
    // lib.optionalAttrs uploader {
      uploader = "${ixUsage}/bin/ix-usage";
    };
  specBaseFile = (pkgs.formats.json {}).generate "ix-usage-spec-${id}.json" specBase;
in
  pkgs.runCommand "${id}-with-usage"
  {
    nativeBuildInputs = [pkgs.makeBinaryWrapper pkgs.jq];
    meta = pkg.meta or {};
    passthru = {unwrapped = pkg;};
  }
  ''
    mkdir -p "$out/bin" "$out/share/ix-usage"
    found=0
    for bin in ${pkg}/bin/*; do
      if [ -f "$bin" ] && [ -x "$bin" ]; then
        found=1
        name=$(basename "$bin")
        spec="$out/share/ix-usage/$name.json"
        jq --arg target "$bin" '. + {target: $target}' ${specBaseFile} > "$spec"
        makeBinaryWrapper ${ixWrap}/bin/ix-wrap "$out/bin/$name" \
          --inherit-argv0 \
          --set IX_USAGE_SPEC "$spec"
      fi
    done
    if [ "$found" != 1 ]; then
      echo "withUsage: ${id} has no executables in bin/" >&2
      exit 1
    fi
  ''
