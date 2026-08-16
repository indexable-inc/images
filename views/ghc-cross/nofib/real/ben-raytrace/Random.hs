{-# LANGUAGE CPP #-}
{-# LANGUAGE BangPatterns #-}

module Random (GenState, mkGenState) where

#if defined(RNG_LEHMER64_MUT)
import Random.Lehmer64Mut
#elif defined(RNG_LEHMER64)
import Random.Lehmer64
#elif defined(RNG_WYHASH64)
import Random.Wyhash64
#else

import System.Random.Stateful
import Data.Word

type Gen = StateGenM StdGen
type GenState = StdGen

mkGenState :: Word64 -> GenState
mkGenState = mkStdGen . fromIntegral

#endif
