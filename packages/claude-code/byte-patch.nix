# One cacheable derivation per equal-length byte mapping applied to the Claude
# Code Bun single-file binary. `input` is a single binary file (the fetched
# download or a previous layer's output) and `$out` is the patched file, so a
# fold of layers forms a DAG in which editing layer N+1 never rebuilds layer N.
#
# Layers stay raw (dontStrip/dontFixup): stripping rewrites the file length and
# corrupts the trailer Bun appends to its single-file executables. Interpreter
# patching (Linux) and ad-hoc re-signing (darwin -- the byte edit invalidates
# the vendor's Developer-ID signature, and AMFI SIGKILLs an unsigned Mach-O)
# happen ONCE at the consuming wrapper leaf, where the binary lands runnable.
# See ./patch-binary.py for the equal-length + occurrence-count gates every rule
# must pass.
{
  runCommand,
  python3,
}: {
  name,
  input,
  rules,
}:
runCommand "claude-code-patch-${name}"
{
  nativeBuildInputs = [python3];
  dontStrip = true;
  dontFixup = true;
  # The mapping rides the derivation env as JSON (the patcher's input format),
  # so rules stay typed Nix at the call site instead of a serialized file.
  mappingJson = builtins.toJSON rules;
  passAsFile = ["mappingJson"];
}
''
  python3 ${./patch-binary.py} ${input} "$mappingJsonPath" $out
''
