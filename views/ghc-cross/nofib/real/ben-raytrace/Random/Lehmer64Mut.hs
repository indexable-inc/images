{-# LANGUAGE MagicHash #-}
{-# LANGUAGE UnboxedTuples #-}
{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE FlexibleInstances #-}
{-# LANGUAGE MultiParamTypeClasses #-}
{-# OPTIONS_GHC -fobject-code #-}

module Random.Lehmer64Mut (Gen, gen, GenState, mkGenState) where

import System.Random.Stateful
import Control.Monad.ST
import Control.Monad.Trans.Reader (ReaderT, ask)
import Control.Monad.Trans.Class
import Data.Primitive.ByteArray
import GHC.Word
import GHC.Prim (timesWord2#)

type Gen = Lehmer64Gen
type GenState = Lehmer64State

gen :: Gen
gen = Lehmer64Gen

mkGenState :: Word64 -> ST s (GenState s)
mkGenState x = do
    ba <- newByteArray 16
    writeByteArray ba 0 x
    writeByteArray ba 1 x
    return $ Lehmer64State ba

data Lehmer64Gen = Lehmer64Gen
data Lehmer64State s = Lehmer64State !(MutableByteArray s)

timesWord2 :: Word -> Word -> (Word, Word)
timesWord2 (W# a) (W# b) =
  case timesWord2# a b of (# x, y #) -> (W# x, W# y)

instance StatefulGen Lehmer64Gen (ReaderT (Lehmer64State s) (ST s)) where
  uniformWord64 Lehmer64Gen = do
      Lehmer64State ba <- ask
      l :: Word64 <- lift $ readByteArray ba 0
      h :: Word64 <- lift $ readByteArray ba 1
      let c = 0xda94042e4dd58b5
          (!x, !y) = timesWord2 (fromIntegral c) (fromIntegral l)
          l' = fromIntegral y :: Word64
          h' = fromIntegral x + h * c
      writeByteArray ba 0 l'
      writeByteArray ba 1 h'
      return h'
  uniformShortByteString _ Lehmer64Gen = error "uniformShortByteString"
