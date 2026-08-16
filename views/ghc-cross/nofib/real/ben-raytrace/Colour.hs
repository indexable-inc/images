{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE PatternSynonyms #-}

module Colour
  ( Colour(Colour)
  , colourToVec3
  , cRed, cGreen, cBlue
  , black, white, red, green, blue
  , gray
  , addColours
  , scaleValue
  , attenuate
  , Channel(..)
  , getChannel
  , averageColours
  ) where

import Data.Ix
import Data.Foldable

import Vector

newtype Colour
  = Colour_ { colourToVec3 :: Vec3 }
  deriving (Show)

pattern Colour { cRed, cGreen, cBlue } = Colour_ (Vec3 cRed cGreen cBlue)
{-# COMPLETE Colour #-}

black, white, red, green, blue :: Colour
black = Colour 0 0 0
white = Colour 1 1 1
red   = Colour 1 0 0
green = Colour 0 1 0
blue  = Colour 0 0 1

gray :: Double -> Colour
gray x = Colour x x x

addColours :: Colour -> Colour -> Colour
addColours (Colour_ a) (Colour_ b) = Colour_ (a `addV` b)

scaleValue :: Double -> Colour -> Colour
scaleValue s (Colour_ c) = Colour_ (s `scaleV` c)

attenuate :: Vec3 -> Colour -> Colour
attenuate atten (Colour_ c) = Colour_ (atten `productV` c)

data Channel
  = Red | Green | Blue
  deriving (Eq, Ord, Show, Enum, Bounded, Ix)

getChannel :: Colour -> Channel -> Double
getChannel (Colour{..}) channel =
  case channel of
    Red -> cRed
    Green -> cGreen
    Blue -> cBlue

averageColours :: [Colour] -> Colour
averageColours xs = Colour_ $ recip (realToFrac n) `scaleV` colour
  where
    Acc colour n = foldl' f (Acc zeroV 0) xs
    f (Acc a n) (Colour_ b) = Acc (a `addV` b) (n+1)

data Acc = Acc !Vec3 !Int
