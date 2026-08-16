{-# LANGUAGE CPP #-}
{-# LANGUAGE GeneralizedNewtypeDeriving #-}
{-# LANGUAGE RankNTypes #-}
{-# LANGUAGE TypeSynonymInstances #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE MultiParamTypeClasses #-}
{-# LANGUAGE FlexibleInstances #-}

module SamplerMonad.Naive
    ( Gen, gen
    , SamplerM, runSamplerM, liftST
    , oneShotState
    ) where

import GHC.Magic (oneShot)
import Data.Word
import Control.Monad.ST
import Control.Monad.Trans.State.Strict
import Control.Monad.Trans.Class
import System.Random.Stateful

import Random

newtype SamplerM s a = SamplerM (StateT GenState (ST s) a)
  deriving (Functor, Applicative, Monad)

data Gen = SamplerGen

gen :: Gen
gen = SamplerGen

instance StatefulGen Gen (SamplerM s) where
  uniformWord64 _ = SamplerM $ StateT $ pure . genWord64
  uniformShortByteString = undefined

runSamplerM
    :: Word64 -> (forall s. SamplerM s a) -> a
runSamplerM seed sampler =
    runST $ case sampler of SamplerM action -> evalStateT action (mkGenState seed)

liftST :: ST s a -> SamplerM s a
liftST = SamplerM . lift
{-# INLINE liftST #-}

oneShotState :: SamplerM s a -> SamplerM s a
oneShotState (SamplerM action) = SamplerM $ StateT $ oneShot $ \s -> runStateT action s
{-# INLINE oneShotState #-}
