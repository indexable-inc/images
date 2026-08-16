`terminfo` [![Build Status](https://travis-ci.org/judah/terminfo.png?branch=master)](https://travis-ci.org/judah/terminfo)
==========

Haskell bindings to the `terminfo` library. See `terminfo`'s [Hackage page](http://hackage.haskell.org/package/terminfo) for more details.

Building
--------

* `terminfo` requires a `configure` script; to build it, run `autoreconf -i`.
* Build `terminfo` with either `stack build` or `cabal build`.

Building on Windows
-------------------

While using this package on Windows is not officially supported,
users wanting to build it on Windows might find [this
ticket](https://github.com/judah/terminfo/issues/46) helpful.
