{-# LANGUAGE NamedFieldPuns #-}
{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE BangPatterns #-}
{-# OPTIONS_GHC -fmax-worker-args=15 #-}

module BoundingBox
  ( BoundingBox, bbMin, bbMax
    -- * Construction
  , emptyBoundingBox
  , mkBoundingBox
  , boundingBoxFromPoints
  , boundingBoxFromBoxes
    -- * Manipulation
  , expandBoundingBox
    -- * Queries
  , boundingBoxContains
  , boundingBoxHits
  ) where

import Vector
import Ray

data BoundingBox
  = BoundingBox { bbMin, bbMax :: {-# UNPACK #-} !(Pt Vec3) }
  deriving (Show)

emptyBoundingBox :: BoundingBox
emptyBoundingBox = BoundingBox originP originP

mkBoundingBox :: Pt Vec3 -> Pt Vec3 -> BoundingBox
mkBoundingBox = BoundingBox

boundingBoxFromPoints :: [Pt Vec3] -> BoundingBox
boundingBoxFromPoints points = BoundingBox {..}
  where
    bbMin = Pt $ Vec3 x y z
      where
        x = minimum $ map (vec3X . getPt) points
        y = minimum $ map (vec3Y . getPt) points
        z = minimum $ map (vec3Z . getPt) points
    bbMax = Pt $ Vec3 x y z
      where
        x = maximum $ map (vec3X . getPt) points
        y = maximum $ map (vec3Y . getPt) points
        z = maximum $ map (vec3Z . getPt) points

boundingBoxFromBoxes :: [BoundingBox] -> BoundingBox
boundingBoxFromBoxes bboxes = boundingBoxFromPoints
  [ p | bbox <- bboxes, p <- [ bbMin bbox, bbMax bbox] ]

expandBoundingBox :: Double  -- ^ expansion factor
                  -> BoundingBox -> BoundingBox
expandBoundingBox s (BoundingBox {bbMin, bbMax}) =
  BoundingBox { bbMin = bbMin `addP` negateV expand
              , bbMax = bbMax `addP` expand
              }
  where
    size = bbMax `subP` bbMin
    expand = (s - 1) `scaleV` size

boundingBoxContains :: BoundingBox -> Pt Vec3 -> Bool
boundingBoxContains (BoundingBox (Pt (Vec3 x0 y0 z0)) (Pt (Vec3 x1 y1 z1)))
                    (Pt (Vec3 x y z)) =
     x0 <= x && x <= x1
  && y0 <= y && y <= y1
  && z0 <= z && z <= z1

{-# INLINE boundingBoxHits #-}
boundingBoxHits :: BoundingBox -> Ray3 -> Bool
boundingBoxHits (BoundingBox {bbMin = Pt bbMin, bbMax = Pt bbMax})
                (Ray3 {direction, start = Pt start}) =
    findInt vec3X (findInt vec3Y (findInt vec3Z (\_ _ -> True))) 0 (1/0)
  where
    {-# INLINE findInt #-}
    findInt :: (Vec3 -> Double)
            -> (Double -> Double -> Bool)
            ->  Double -> Double -> Bool
    findInt f k !tmin !tmax
        | tmax' <= tmin' = False
        | otherwise      = k tmin' tmax'
      where
        a = (f bbMin - f start) / f direction
        b = (f bbMax - f start) / f direction
        t0 = min a b
        t1 = max a b
        tmin' = max t0 tmin
        tmax' = min t1 tmax

