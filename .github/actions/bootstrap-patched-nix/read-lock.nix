let
  field = builtins.getEnv "BOOTSTRAP_LOCK_FIELD";
  lockFile = builtins.getEnv "BOOTSTRAP_LOCK";
  lock = builtins.fromJSON (builtins.readFile lockFile);
  value = lock.${field} or (throw "bootstrap-patched-nix: lock has no `${field}` field");
in
  if lockFile == ""
  then throw "bootstrap-patched-nix: BOOTSTRAP_LOCK is empty"
  else if field == ""
  then throw "bootstrap-patched-nix: BOOTSTRAP_LOCK_FIELD is empty"
  else if builtins.isString value && value != ""
  then value
  else throw "bootstrap-patched-nix: lock field `${field}` must be a non-empty string"
