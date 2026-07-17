# The orphan-cleanup tests probe OS process state with pgrep, which the Nix
# sandbox does not provide; they still run in any normal dev environment.
exclude = if System.find_executable("pgrep"), do: [], else: [:os_procs]
ExUnit.start(exclude: exclude)
