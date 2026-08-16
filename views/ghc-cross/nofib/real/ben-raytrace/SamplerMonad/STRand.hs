{-# LANGUAGE RankNTypes #-}
{-# LANGUAGE GeneralizedNewtypeDeriving #-}

module SamplerMonad.STRand
    ( Gen, gen
    , SamplerM, runSamplerM, liftST
    , oneShotState
    ) where

import Control.Monad.ST
import Control.Monad.Trans.Reader
import Control.Monad.Trans.Class
import Control.Monad.Reader.Class
import System.Random.Stateful
import GHC.Magic
import Data.Word

import Random.Lehmer64Mut

newtype SamplerM s a = SamplerM (ReaderT (GenState s) (ST s) a)
  deriving (Functor, Applicative, Monad, MonadReader (GenState s), StatefulGen Gen)

runSamplerM
    :: Word64 -> (forall s. SamplerM s a) -> a
runSamplerM seed sampler = runST $ do
    s <- mkGenState seed
    case sampler of SamplerM action -> runReaderT action s

liftST :: ST s a -> SamplerM s a
liftST = SamplerM . lift
{-# INLINE liftST #-}

oneShotState :: SamplerM s a -> SamplerM s a
oneShotState (SamplerM action) =
    SamplerM $ ReaderT $ oneShot $ \s -> runReaderT action s
{-# INLINE oneShotState #-}
