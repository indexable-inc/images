# The orphan-cleanup tests probe OS process state with pgrep, which the Nix
# sandbox does not provide; they still run in any normal dev environment.
# The local-TUI tests need the compiled :tui_ex app (IX_MCP_TUI_EX); the nix
# check always provides it, so CI cannot rot while a plain local `mix test`
# skips them visibly.
exclude =
  if System.find_executable("pgrep"), do: [], else: [:os_procs]

exclude =
  if System.get_env("IX_MCP_TUI_EX"), do: exclude, else: [:tui_local | exclude]

# Same contract for the Gmail binding (IX_MCP_GMAIL_EX).
exclude =
  if System.get_env("IX_MCP_GMAIL_EX"), do: exclude, else: [:gmail_ex | exclude]

ExUnit.start(exclude: exclude)
