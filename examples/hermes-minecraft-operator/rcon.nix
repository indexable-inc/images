# The RCON contract both nodes share: the minecraft node seeds this
# password into the file the server reads, and the hermes node hands it
# to the MCP server. One definition so they cannot drift.
#
# Committed plaintext is deliberate and matches the survival example's
# forwarding secret: RCON is only reachable inside this fleet's
# east-west group, and the value is obviously a change-me. Rotate it by
# editing here, `ix fleet switch`, and deleting
# /var/lib/minecraft/.ix-rcon-password on the minecraft node (the seed
# only writes when the file is absent).
{
  port = 25575;
  password = "ix-hermes-operator-rcon-change-me";
}
