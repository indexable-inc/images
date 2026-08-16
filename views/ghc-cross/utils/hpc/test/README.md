# Testsuite

## Directory structure

The subdirectory `test/GHC/` contains the tests which also exist in that form in the
GHC testsuite. The other directories correspond to the various subcommands that `hpc-bin` provides.

## Running the tests

The testsuite expects the two environment variables `HPC` and `GHC` to be set.
For example, the testsuite can be invoked with:

```console
HPC=hpc GHC=ghc cabal test
```
