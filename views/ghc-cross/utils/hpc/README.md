# Haskell Program Coverage

This repository provides the `hpc` binary which produces coverage reports in
various formats from .tix and .mix files.

## Documentation

The documentation of the `hpc-binary` is available online on [readthedocs.io](https://hpc-bin.readthedocs.io/en/latest/index.html).
In order to build the documentation locally, you have to install the [sphinx](https://www.sphinx-doc.org/en/master/) documentation generator on your machine, as well as the `sphinx_rtd_theme`, and invoke `make html` in the `docs/` subdirectory.

## Contributing

The easiest way to get started is to load the provided [development
container](https://code.visualstudio.com/docs/remote/containers) into Visual
Studio Code. This container is based on the [official Haskell
devcontainer](https://github.com/microsoft/vscode-dev-containers/tree/main/containers/haskell/.devcontainer)
with some minor tweaks like automatic file formatting on save (provided by
Ormolu).

