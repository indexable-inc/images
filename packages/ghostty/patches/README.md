# ghostty patches

Empty for now (index#3768 vendors the fork source and darwin build only).
The surface-teardown fix -- kill the child process group and `waitpid`-reap
on `close_surface`, honoring `undo-timeout` -- lands here as a follow-up
patch in this series, generated with `nix run .#rebase-patches -- ghostty`.
