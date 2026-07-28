/**
The demo workload binary. Nothing here says "container": it is an ordinary
nix-built application, and job.nix points raw_exec at its store path.
*/
{
  ix,
  pkgs,
}:
ix.writePythonApplication pkgs {
  name = "whoami-http";
  src = ./whoami.py;
}
