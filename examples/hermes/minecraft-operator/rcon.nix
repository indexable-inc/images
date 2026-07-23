# The RCON contract both VMs share: the minecraft VM seeds this
# password into the file the server reads, and the hermes VM hands it
# to the MCP server. One definition so they cannot drift.
#
# Committed plaintext is deliberate and matches the survival example's
# forwarding secret: RCON is only reachable inside this example's
# east-west group, and the value is obviously a change-me. Rotate it by
# editing here, re-running `ix apply .#minecraft`, and deleting
# /var/lib/minecraft/.ix-rcon-password on the minecraft VM (the seed
# only writes when the file is absent).
{
  port = 25575;
  password = "ix-hermes-operator-rcon-change-me";
}
