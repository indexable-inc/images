{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE BangPatterns #-}

module BVH
  ( BVH(..), mkBVH
  , showBVH
  , bvhFigure
  ) where

import Data.Ord (comparing)
import Data.List (sortBy)
import Control.Monad (guard)
import Control.Applicative

import BoundingBox
import Figure
import Interval
import RandomDist (choose)
import Ray
import SamplerMonad
import Vector

data BVH
  = BVHNode { bvhBox_ :: {-# UNPACK #-} !BoundingBox
            , childA, childB :: !BVH
            }
  | BVHLeaf !Figure

showBVH :: BVH -> String
showBVH = unlines . go 0
  where
    go n (BVHNode box a b) =
      [indent n $ show box] ++ go (n+1) a ++ go (n+1) b
    go n (BVHLeaf fig) =
      [indent n $ "Leaf: " ++ show (boundingBox fig)]

    indent n s = replicate (2*n) ' ' ++ s

bvhBoundingBox :: BVH -> BoundingBox
bvhBoundingBox (BVHNode bbox _ _) = bbox
bvhBoundingBox (BVHLeaf fig)      = boundingBox fig

bvhFigure :: BVH -> Figure
bvhFigure bvh = Figure {boundingBox = bvhBoundingBox bvh, ..}
  where
    hitTest ray hitInterval = hitBVH ray hitInterval bvh

hitBVH :: Ray3 -> Interval -> BVH -> Maybe Hit
hitBVH = go
  where
    go !ray !hitInterval (BVHNode bbox ca cb) = do
      guard $ boundingBoxHits bbox ray
      let hitA = go ray hitInterval ca
          hitInterval' = case hitA of
            Just ha -> hitInterval { iUpper = hitDistance ha }
            Nothing -> hitInterval
      let hitB = go ray hitInterval'  cb
      hitB <|> hitA

    go ray hitInterval (BVHLeaf fig) =
      hitTest fig ray hitInterval

mkBVH :: [Figure] -> SamplerM s BVH
mkBVH []  = error "mkBVH: no figures"
--mkBVH [x] = pure $ BVHLeaf x
mkBVH xs | length xs < 5 = pure $ BVHLeaf $ mconcat xs
mkBVH xs  = do
  axis <- choose [(1, pure vec3X), (1, pure vec3Y), (1, pure vec3Z)] gen
  let sorted = sortBy (comparing $ axis . getPt . bbMin . boundingBox) xs
  let (as,bs) = splitAt (length xs `div` 2) sorted
  childA <- mkBVH as
  childB <- mkBVH bs
  let bbox = boundingBoxFromBoxes $ map bvhBoundingBox [childA, childB]
  return $ BVHNode bbox childA childB
