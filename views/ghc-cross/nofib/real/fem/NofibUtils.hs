{-# LANGUAGE CPP #-}

module NofibUtils where

import Data.Char (ord)
import Data.List (foldl')

import System.Environment (getArgs)

-- | A very simple hash function so that we don't have to store and compare
-- huge output files.
hash :: String -> Int
hash str = foldl' (\acc c -> ord c + acc*31) 0 str
{-# INLINE hash #-}
{-
Note: Originally, `hash` was eta-reduced and not explicitly inlined, and as a
result the `foldl'` call here was not saturated.  Thus the definition of `hash`
was simple enough to inline into the call site, where the provided string
allowed `foldl'` to inline into `foldr`, which then enabled fusion, avoiding
potentially non-trivial allocation overhead.

In !5259, we're reducing the arity of `foldl'`, so that it can inline with just
two arguments.  This yields performance improvements in common idiomatic code,
see !5259 for details.

That reduced arity makes `foldl'` inline into even the eta-reduced `hash`,
which then (for lack of an INLINE here) also inlined `foldr` and the fusion
opportunity was lost.

Quoting Simon, <https://gitlab.haskell.org/ghc/ghc/-/merge_requests/5259#note_341945>

    * let's add that INLINE to hash in NofibUtils. We don't usually mess with
      nofib, but we don't want to perpetuate reliance on a fluke.

In addition, eta-expanding `hash`.  It will now saturate both the original
and the post-!5259 `foldl'`.
-}

-- | Using @salt xs@ on an loop-invariant @xs@ inside a loop prevents the
-- compiler from floating out the input parameter.
#ifdef __GLASGOW_HASKELL__
salt :: a -> IO a
salt = pure
{-# NOINLINE salt #-}
#else
salt :: [a] -> IO [a]
-- this won't work with real/lift, but I can't think of another way
salt xs = do
  s <- length <$> getArgs
  -- Invariant: There are less than 'maxBound' parameters passed to the
  --            executable, otherwise this isn't really 'pure'
  --            anymore.
  pure (take (max (maxBound - 1) s) xs)
#endif
