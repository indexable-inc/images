module Matrix where

import Vector

data Mat3
  = Mat3 { mat3R1, mat3R2, mat3R3 :: !Vec3 }
  deriving (Show)

ident3 :: Mat3
ident3 = Mat3 a b c
  where
    a = Vec3 1 0 0
    b = Vec3 0 1 0
    c = Vec3 0 0 1

scaleM3 :: Double -> Mat3 -> Mat3
scaleM3 s (Mat3 a b c) = Mat3 (f a) (f b) (f c)
  where
    f = (s `scaleV`)

mat3Add :: Mat3 -> Mat3 -> Mat3
mat3Add a b =
    Mat3 (mat3R1 a `addV` mat3R1 b)
         (mat3R2 a `addV` mat3R2 b)
         (mat3R3 a `addV` mat3R3 b)

mat3App :: Mat3 -> Vec3 -> Vec3
mat3App (Mat3 c1 c2 c3) v =
    Vec3 x y z
  where
    x = c1 `dotV` v
    y = c2 `dotV` v
    z = c3 `dotV` v

rotationMat3
  :: Vec3   -- ^ axis
  -> Double -- ^ angle
  -> Mat3
rotationMat3 (Vec3 x y z) theta = Mat3 a b c
  where
    a = Vec3 (f x)          (g x y (-z))     (g x z y)
    b = Vec3 (g y x z)      (f y)            (g y z (-x))
    c = Vec3 (g z x (-y))   (g z y x)        (f z)

    f alpha = cos_th + alpha**2 * (1 - cos_th)
    g alpha beta gamma = alpha * beta * (1 - cos_th) + gamma * sin_th

    sin_th = sin theta
    cos_th = cos theta
