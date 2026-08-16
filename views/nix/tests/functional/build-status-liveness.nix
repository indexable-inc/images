{
  mode,
  bigParallel ? false,
}:

with import ./config.nix;

mkDerivation {
  name = "build-status-liveness-${mode}";
  requiredSystemFeatures = if bigParallel then [ "big-parallel" ] else [ ];
  buildCommand =
    if mode == "active" then
      ''
        end=$((SECONDS + 3))
        while (( SECONDS < end )); do :; done
        echo active > "$out"
      ''
    else if mode == "disconnect" then
      ''
        end=$((SECONDS + 30))
        while (( SECONDS < end )); do :; done
        echo disconnect > "$out"
      ''
    else
      ''
        sleep ${if bigParallel then "2" else "10"}
        echo silent > "$out"
      '';
}
