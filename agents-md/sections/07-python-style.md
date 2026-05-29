## Python style

Default repo-owned Python apps to uv: `pyproject.toml`, committed `uv.lock`,
normal `src/<package>/` files, and Nix packaging through
[`ix.buildUvApplication`](lib/build-uv-application.nix).

Use [`ix.writePythonApplication`](lib/default.nix) for tiny single-file commands
without PyPI dependencies or multiple source files. Once a script needs
dependencies, console entry points, or a package layout, give it the uv project
shape.

The Python helpers run `ty` by default. Disable the check only when the package
has a deliberate reason.
