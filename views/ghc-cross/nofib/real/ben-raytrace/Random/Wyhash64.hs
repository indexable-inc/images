{-# LANGUAGE CPP #-}
{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE FlexibleInstances #-}
{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE UndecidableInstances #-}
{-# LANGUAGE MultiParamTypeClasses #-}
{-# LANGUAGE UnboxedTuples #-}
{-# LANGUAGE MagicHash #-}
{-# OPTIONS_GHC -fobject-code #-}

module Random.Wyhash64 (GenState, mkGenState) where

import System.Random.Stateful
import Data.Bits
import GHC.Word
import GHC.Prim (timesWord2#)

type GenState = Wyhash64State

mkGenState :: Word64 -> GenState
mkGenState x = Wyhash64State x

data Wyhash64State = Wyhash64State !Word64

timesWord2 :: Word64 -> Word64 -> (Word64, Word64)
timesWord2 (W64# a) (W64# b) =
  case timesWord2# a b of (# x, y #) -> (W64# x, W64# y)

instance RandomGen Wyhash64State where
  split = error "Lehmer64 is not splittable"
  genWord64 (Wyhash64State x) =
    let (tmp1_h, tmp1_l) = timesWord2 x c2
        m1 = tmp1_h `xor` tmp1_l :: Word64
        (tmp2_h, tmp2_l) = timesWord2 m1 c3
        m2 = tmp2_h `xor` tmp2_l
    in (m2, Wyhash64State (x + c1))
    where
      c1 = 0x60bee2bee120fc15
      c2 = 0xa3b195354a39b70d
      c3 = 0x1b03738712fad5c9
