module Mesh where

import Vector
import Matrix

data Triangle = Triangle { triA, triB, triC :: !(Pt Vec3) }
  deriving (Show)

newtype Mesh = Mesh { meshTriangles :: [Triangle] }

transformMesh :: (Pt Vec3 -> Pt Vec3) -> Mesh -> Mesh
transformMesh f (Mesh pts) = Mesh $ map g pts
  where
    g tri =
      tri { triA = f $ triA tri
          , triB = f $ triB tri
          , triC = f $ triC tri
          }

translateMesh :: Vec3 -> Mesh -> Mesh
translateMesh v = transformMesh (`addP` v)

rotateMesh :: Mat3 -> Mesh -> Mesh
rotateMesh m = transformMesh (Pt . (m `mat3App`) . getPt)

xyQuadMesh :: Vec2
           -> Mesh
xyQuadMesh (Vec2 sx sy) =
    Mesh [ Triangle p1 p0 p2, Triangle p1 p2 p3 ]
  where
    p0 = Pt $ Vec3 (-sx) (-sy) 0
    p1 = Pt $ Vec3 (-sx) ( sy) 0
    p2 = Pt $ Vec3 ( sx) (-sy) 0
    p3 = Pt $ Vec3 ( sx) ( sy) 0
