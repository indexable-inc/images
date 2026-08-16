{-# OPTIONS_GHC -fobject-code #-}
{-# LANGUAGE RankNTypes #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE UnboxedTuples #-}
{-# LANGUAGE MagicHash #-}
{-# LANGUAGE TypeSynonymInstances #-}
{-# LANGUAGE MultiParamTypeClasses #-}
{-# LANGUAGE FlexibleInstances #-}

module SamplerMonad.Unboxed
    ( Gen, gen
    , SamplerM, runSamplerM, liftST
    , oneShotState
    ) where

import Control.Monad (ap)
import Control.Monad.ST
import Data.Word
import GHC.Magic (oneShot, runRW#)
import GHC.ST hiding (liftST)
import GHC.Prim (State#)
import System.Random.Stateful

import Random

data SamplerM s a = SamplerM (State# s -> GenState -> (# State# s, GenState, a #))

data Gen = SamplerGen

gen :: Gen
gen = SamplerGen

instance StatefulGen Gen (SamplerM s) where
  uniformWord64 _ = SamplerM $ \s gen ->
    case genWord64 gen of
      (w, gen') -> (# s, gen', w #)
  uniformShortByteString = undefined

runSamplerM
    :: Word64 -> (forall s. SamplerM s a) -> a
runSamplerM seed (SamplerM f) =
    runRW# (oneShot $ \s0 ->
      case f s0 (mkGenState seed) of
        (# _s1, !_gen1, x #) -> x)

instance Functor (SamplerM s) where
  fmap f (SamplerM g) = SamplerM (oneShot $ \s0 -> oneShot $ \ !gen0 ->
    case g s0 gen0 of
      (# s1, gen1, x #) -> (# s1, gen1, f x #))

instance Applicative (SamplerM s) where
  pure x = SamplerM (oneShot $ \s0 -> oneShot $ \ !gen0 -> (# s0, gen0, x #))
  (<*>) = ap

instance Monad (SamplerM s) where
  return = pure
  SamplerM f >>= g = SamplerM (oneShot $ \s0 -> oneShot $ \ !gen0 ->
    case f s0 gen0 of
      (# s1, !gen1, x #) ->
        case g x of
          SamplerM h -> h s1 gen1)

liftST :: ST s a -> SamplerM s a
liftST (ST f) = SamplerM (oneShot $ \s0 -> oneShot $ \ !gen0 ->
    case f s0 of
      (# s1, x #) -> (# s1, gen0, x #))
{-# INLINE liftST #-}

oneShotState :: SamplerM s a -> SamplerM s a
oneShotState (SamplerM action) =
    SamplerM $ oneShot $ \s0 -> oneShot $ \gen0 -> action s0 gen0
{-# INLINE oneShotState #-}
