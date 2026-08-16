{-# LANGUAGE RecordWildCards #-}

module Interval where

-- | A half-open interval.
data Interval
  = Interval { iLower, iUpper :: !Double }

inInterval :: Interval -> Double -> Bool
inInterval (Interval{..}) a = a >= iLower && a < iUpper

