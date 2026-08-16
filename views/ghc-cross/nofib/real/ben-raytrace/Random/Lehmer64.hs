{-# LANGUAGE CPP #-}
{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE FlexibleInstances #-}
{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE UndecidableInstances #-}
{-# LANGUAGE MultiParamTypeClasses #-}
{-# LANGUAGE UnboxedTuples #-}
{-# LANGUAGE MagicHash #-}
{-# OPTIONS_GHC -fobject-code #-}

module Random.Lehmer64 (GenState, mkGenState) where

import System.Random.Stateful
import GHC.Word
import GHC.Prim (timesWord2#)

type GenState = Lehmer64State

mkGenState :: Word64 -> GenState
mkGenState x = Lehmer64State x x

data Lehmer64State = Lehmer64State !Word64 !Word64

timesWord2 :: Word -> Word -> (Word, Word)
timesWord2 (W# a) (W# b) =
  case timesWord2# a b of (# x, y #) -> (W# x, W# y)

instance RandomGen Lehmer64State where
  split = error "Lehmer64 is not splittable"
  genWord64 (Lehmer64State h l) =
    let c = 0xda94042e4dd58b5
        (!x, !y) = timesWord2 (fromIntegral c) (fromIntegral l)
        l' = fromIntegral y
        h' = fromIntegral x + h * c
    in (h, Lehmer64State h' l')
