module Vector where

infixl 6 `addV`, `subV`
infixl 7 `scaleV`, `dotV`

class VectorSpace v where
  addV       :: v -> v -> v
  subV       :: v -> v -> v
  negateV    :: v -> v
  scaleV     :: Double -> v -> v
  dotV       :: v -> v -> Double
  productV   :: v -> v -> v
  zeroV      :: v

  normV      :: v -> Double
  normV = sqrt . quadranceV

  quadranceV :: v -> Double
  quadranceV u = dotV u u

  normaliseV :: v -> v
  normaliseV v = recip (normV v) `scaleV` v

  addP :: Pt v -> v -> Pt v
  addP (Pt u) v = Pt (u `addV` v)

  subP :: Pt v -> Pt v -> v
  subP (Pt u) (Pt v) = u `subV` v

  originP :: Pt v
  originP = Pt zeroV

newtype Pt v
  = Pt { getPt :: v }
  deriving (Show)

data Vec3
  = Vec3 { vec3X, vec3Y, vec3Z :: !Double }
  deriving (Show)

crossV3 :: Vec3 -> Vec3 -> Vec3
crossV3 (Vec3 x0 y0 z0) (Vec3 x1 y1 z1) =
    Vec3 x y z
  where
    x = y0 * z1 - z0 * y1
    y = z0 * x1 - x0 * z1
    z = x0 * y1 - y0 * x1

instance VectorSpace Vec3 where
  addV (Vec3 x y z) (Vec3 a b c) = Vec3 (x+a) (y+b) (z+c)
  subV (Vec3 x y z) (Vec3 a b c) = Vec3 (x-a) (y-b) (z-c)
  negateV (Vec3 x y z) = Vec3 (negate x) (negate y) (negate z)
  scaleV s (Vec3 x y z) = Vec3 (s*x) (s*y) (s*z)
  dotV (Vec3 x y z) (Vec3 a b c) = x*a + y*b + z*c
  productV (Vec3 x y z) (Vec3 a b c) = Vec3 (x*a) (y*b) (z*c)
  zeroV = Vec3 0 0 0

data Vec2
  = Vec2 !Double !Double
  deriving (Show)

instance VectorSpace Vec2 where
  addV (Vec2 x y) (Vec2 a b) = Vec2 (x+a) (y+b)
  subV (Vec2 x y) (Vec2 a b) = Vec2 (x-a) (y-b)
  negateV (Vec2 x y) = Vec2 (negate x) (negate y)
  scaleV s (Vec2 x y) = Vec2 (s*x) (s*y)
  dotV (Vec2 x y) (Vec2 a b) = x*a + y*b
  productV (Vec2 x y) (Vec2 a b) = Vec2 (x*a) (y*b)
  zeroV = Vec2 0 0

