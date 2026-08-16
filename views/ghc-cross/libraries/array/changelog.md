# Changelog for [`array` package](http://hackage.haskell.org/package/array)

## 0.5.8.0 *Aug 2024*

### Added

  * Folds for arrays: `foldrArray`, `foldlArray'`, `foldlArray`, `foldrArray'`,
    `traverseArray_`, `forArray_`, `foldlArrayM'`, `foldrArrayM'`.
  * Folds for mutable arrays: `foldlMArray'`, `foldrMArray'`, `mapMArrayM_`,
    `forMArrayM_`, `foldlMArrayM'`, `foldrMArrayM'`.

### Fixed

  * Fix a build error that the package can't be buildable before `base-4.14`.

## 0.5.7.0  *April 2024*

### Changed

  * `MArray` now has a `MINIMAL` pragma
  * Optimisation of `newListArray` and `newGenArray`

## 0.5.6.0  *July 2023*

### Changed

  * `listArray` and `newListArray` are now good consumers of the input list
  * Bump base bound to `<4.20`

### Added

  * Add the `genArray` and `newGenArray` function
  * Add `Data.Array.MArray.modifyArray` and `Data.Array.MArray.modifyArray'`
    These are also exposed from `Data.Array.IO`, `Data.Array.ST`, and
    `Data.Array.Storable`.
  * Add `Data.Array.IArray.(!?)`

### Fixed

  * Array docs regarding constructing arrays
  * Update note [Inlining and fusion]
  * Unboxed Bool arrays no longer cause spurious alarms
    when used with `-fcheck-prim-bounds`
  * Replace Haddock hide pragma with not-home to make the Haddocks more readable

## 0.5.5.0  *February 2022*

  * Compatibility with GHC's new JavaScript backend.

## 0.5.4.0  *July 2019*

  * Add a `Read` instance for `UArray`

## 0.5.3.0  *Oct 2018*

  * Bundled with GHC 8.6.2
  * Drop support for GHC versions prior to GHC 8.0

## 0.5.2.0  *Jul 2017*

  * Bundled with GHC 8.2.1
  * Overflow check in `unsafeNewArray` (#229)
  * Fix and simplify handling of `Bool` arrays
  * Export `unsafeFreezeIOUArray` from `Data.Array.IO.Internals`
  * Drop support for GHC versions prior to GHC 7.8

## 0.5.1.1  *Apr 2016*

  * Bundled with GHC 8.0.1
  * Use `@since` syntax in Haddock comments
  * Don't needlessly call `bounds` in `Data.Array.Base.elems` (#10014)

## 0.5.1.0  *Mar 2015*

  * Bundled with GHC 7.10.1
  * Add role annotations for GHC >= 7.8 (#9220)

## 0.5.0.0  *Nov 2013*

  * Update to Cabal 1.10 format
  * Remove NHC and Hugs specific code
  * Remove deprecated function exports `Data.Array.IO.castIOUArray`,
    `Data.Array.MArray.unsafeFreeze`, `Data.Array.MArray.unsafeThaw`,
    and `Data.Array.ST.castSTUArray`; These functions are still
    available from the `Data.Array.Unsafe` module.

## 0.4.0.1  *Sep 2012*

  * Bundled with GHC 7.6.1
  * Fix inline rule shadowing warnings

## 0.4.0.0  *Feb 2012*

  * Bundled with GHC 7.4.1
  * Add support for SafeHaskell
  * New `Data.Array.IO.Safe` module
  * New `Data.Array.MArray.safe` module
  * New `Data.Array.ST.safe` module
  * New `Data.Array.Storable.Internals` module
  * New `Data.Array.Storable.Safe` module
  * New `Data.Array.Unsafe` module
