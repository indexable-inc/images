# NixOS/nix@2c6d06e src/libstore/machines.cc parseBuilderLine reads these eight columns.
{lib}: machine: let
  listField = values:
    if values == []
    then "-"
    else lib.concatStringsSep "," values;
  optionalField = value:
    if value == null
    then "-"
    else toString value;
in
  lib.concatStringsSep " " [
    "${machine.protocol}://${machine.sshUser}@${machine.hostName}"
    (listField machine.systems)
    (optionalField machine.sshKey)
    (toString machine.maxJobs)
    (toString machine.speedFactor)
    (listField machine.supportedFeatures)
    (listField machine.mandatoryFeatures)
    (optionalField machine.publicHostKey)
  ]
