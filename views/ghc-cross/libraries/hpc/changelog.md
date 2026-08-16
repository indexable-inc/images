# Changelog for [`hpc` package](http://hackage.haskell.org/package/hpc)

## 0.7.0.2 *May 2024*

  * Bump upper bounds of dependencies on base and time libraries.

## 0.7.0.1 *January 2024*

  * Bump upper bounds of dependencies on filepath and containers libraries.

## 0.7.0.0 *August 2023* 

  * The function `readMix` no longer uses lazyIO to read `.mix` files, which is consistent with the behaviour of the `readTix` function.
  * The import of the modules from this package is no longer considered safe, since we use functions from the `System.Directory` module from the `directory` package which is no longer considered safe beginning from version `1.3.8`.
  * Replace uses of `String` by `FilePath` in functions `readTix`, `writeTix`, `getTixFileName`, `readMix` and `mixCreate`.
  * Remove support for version of GHC below 8.6

## 0.6.2.0  *September 2021*

  * Addition of `NFData` instances for `Tix` and `TixModule`

## 0.6.1.0  *October 2019*

  * Addition of `readFileUtf8` and `writeFileUtf8` functions.
  * Ensure `.tix` files read and written in UTF-8, regadless of the system locale.

## 0.6.0.3  *May 2016*

  * Bundled with GHC 8.0.1

  * Improved error messages (#10529)

## 0.6.0.2  *Mar 2015*

  * Bundled with GHC 7.10.1

  * Allow same `Mix` file in different dirs (#9619)

## 0.6.0.1  *Mar 2014*

  * Bundled with GHC 7.8.1

  * Update to Cabal 1.10 format

  * Drop support for GHC prior to version 7.2.2

  * Minor improvements to Haddock docs
