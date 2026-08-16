module Ray where

import Vector

data Ray3
  = Ray3 { start :: !(Pt Vec3), direction :: !Vec3 }
  deriving (Show)

pointOnRay3 :: Ray3 -> Double -> Pt Vec3
pointOnRay3 ray t = start ray `addP` (t `scaleV` direction ray)
