{-# LANGUAGE CPP #-}

module SamplerMonad
    ( SamplerM, runSamplerM, liftST
    , Gen, gen
    , oneShotState
    ) where

#if defined(RNG_LEHMER64_MUT)
import SamplerMonad.STRand
#elif defined(UNBOXED_SAMPLER)
import SamplerMonad.Unboxed
#else
import SamplerMonad.Naive
#endif
