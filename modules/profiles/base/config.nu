# System-wide Nushell config shipped by the ix base profile.
# Lives at /etc/nushell/config.nu via programs.nushell.configFile.source.

# Default is `true`, which prints a multi-line ASCII welcome on every SSH
# login. An operator reconnecting to a long-lived VM does not want that.
$env.config.show_banner = false

# Default history is plaintext, ~10k entries, in-memory until shell exit.
# SQLite survives concurrent shells cleanly, gives ranged history queries,
# and the larger size matches a dev VM that an operator reconnects to
# many times across a long session.
$env.config.history = {
  file_format: sqlite
  max_size: 100000
  sync_on_enter: true
  isolation: false
}
