{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE RankNTypes #-}

module Figure
  ( -- * Figures
    Figure(..)
  , sphere
  , triangle
    -- * Hit-testing
  , Hit(..)
  , hitTest
    -- * Materials
  , Material(..)
  , addEmission
  , withConstEmission
  , checkerMaterial
  , normalShadeMaterial
  , distanceShadeMaterial
  , lambertian
  , metal
  , dielectric
  ) where

import Data.List.NonEmpty (NonEmpty(..), toList)
import Data.Semigroup

import BoundingBox
import Colour
import Interval
import RandomDist
import Ray
import SamplerMonad
import Vector

data Figure
  = Figure { boundingBox :: !BoundingBox
           , hitTest     :: !(Ray3 -> Interval -> Maybe Hit)
           }

instance Monoid Figure where
  mempty = Figure { boundingBox = emptyBoundingBox
                  , hitTest = \_ _ -> Nothing
                  }
  mconcat [] = mempty
  mconcat (x:xs) = sconcat (x :| xs)

instance Semigroup Figure where
  fig1 <> fig2 = sconcat (fig1 :| [fig2])
  sconcat figs = Figure { boundingBox = boundingBoxFromBoxes $ map boundingBox $ toList figs
                        , hitTest = hitTestMany (toList figs)
                        }

hitTestMany :: [Figure] -> Ray3 -> Interval -> Maybe Hit
hitTestMany figs ray = go figs Nothing
  where
    go :: [Figure] -> Maybe Hit -> Interval -> Maybe Hit
    go [] accum !_ = accum
    go (fig:rest) accum int =
      case hitTest fig ray int of
        Nothing -> go rest accum int
        Just hit ->
          case accum of
            Nothing
                -> go rest (Just hit) (int {iUpper=hitDistance hit})
            Just oldHit
              | hitDistance hit < hitDistance oldHit
                -> go rest (Just hit) (int {iUpper=hitDistance hit})
              | otherwise
                -> go rest accum int

data Hit
  = Hit { hitDistance :: !Double
        , hitPoint    :: !(Pt Vec3)
        , hitNormal   :: !Vec3 -- ^ unit vector normal to surface
        , hitUV       :: !(Pt Vec2)
        , hitMaterial :: !Material
        }

sphere :: Pt Vec3 -> Double -> Material -> Figure
sphere !center !radius !material = Figure {..}
  where
    boundingBox =
      let r = 1.0001*radius
          v = Vec3 r r r
      in mkBoundingBox (center `addP` v) (center `addP` negateV v)

    hitTest :: Ray3 -> Interval -> Maybe Hit
    hitTest ray hitInterval
      | discrim <= 0                         = Nothing
      | inInterval hitInterval hit1Distance  = Just $! mkHit hit1Distance
      | inInterval hitInterval hit2Distance  = Just $! mkHit hit2Distance
      | otherwise                            = Nothing
      where
        oc = start ray `subP` center
        a = quadranceV (direction ray)
        b = oc `dotV` direction ray
        c = quadranceV oc - radius*radius
        discrim = b*b - a*c
        hit1Distance = (-b - sqrt discrim) / a
        hit2Distance = (-b + sqrt discrim) / a

        getUV p =
          let phi = atan2 (vec3Z p) (vec3X p)
              theta = asin (vec3Y p)
              u = 1 - (phi + pi) / (2*pi)
              v = (theta + pi/2) / pi
          in Pt (Vec2 u v)

        mkHit dist =
          let hitPoint = pointOnRay3 ray dist
          in Hit { hitDistance = dist
                 , hitPoint    = hitPoint
                 , hitNormal   = recip radius `scaleV` (hitPoint `subP` center)
                 , hitUV       = getUV (recip radius `scaleV` (hitPoint `subP` center))
                 , hitMaterial = material
                 }

triangle :: Pt Vec3 -> Pt Vec3 -> Pt Vec3 -> Material -> Figure
triangle !p0 !p1 !p2 material = Figure {..}
  where
    boundingBox = expandBoundingBox (1+1e-4) $ boundingBoxFromPoints [p0,p1,p2]
    hitTest = moellerTrumbore p0 p1 p2 material

-- | Moeller-Trumbore ray-triangle intersection
moellerTrumbore :: Pt Vec3 -> Pt Vec3 -> Pt Vec3 -> Material
                -> Ray3 -> Interval
                -> Maybe Hit
moellerTrumbore !p0 !p1 !p2 material ray hitInterval
  | a > (-epsilon)
  , a < epsilon                   = Nothing
  | u < 0 || u > 1                = Nothing
  | v < 0 || u+v > 1              = Nothing
  | inInterval hitInterval t      = Just $ Hit { hitDistance = t
                                               , hitPoint = pointOnRay3 ray t
                                               , hitNormal = normaliseV $ edge1 `crossV3` edge2
                                               , hitUV = Pt $ Vec2 u v
                                               , hitMaterial = material
                                               }
  | otherwise                     = Nothing
  where
    edge1 = p1 `subP` p0
    edge2 = p2 `subP` p0
    h = direction ray `crossV3` edge2
    a = edge1 `dotV` h

    f = recip a
    s = start ray `subP` p0
    u = f * (s `dotV` h)

    q = s `crossV3` edge1
    v = f * (direction ray `dotV` q)

    t = f * (edge2 `dotV` q)

    epsilon = 1e-7

data Material
  = Material { scatter :: forall s. ()
                       => Ray3   -- ^ incoming ray
                       -> Hit    -- ^ hit
                       -> SamplerM s (Vec3, Maybe Ray3)  -- ^ attenuation and scattered ray
             , emission :: !Emission
             }

data Emission
  = NoEmission
  | ConstEmission !Colour
  -- | SampleEmission { sampleEmission :: !(forall s. Hit -> SamplerM s Colour) }

addEmission :: Emission -> Colour -> Colour
addEmission NoEmission c = c
addEmission (ConstEmission x) c = c `addColours` x

withConstEmission :: Colour -> Material -> Material
withConstEmission c mat = mat {emission = ConstEmission c}

checkerMaterial :: Vec2 -> Material -> Material -> Material
checkerMaterial (Vec2 sx sy) mat1 mat2 =
    Material {scatter = _scatter, emission = emission }
  where
    _scatter ray hit =
      let Pt (Vec2 x y) = hitUV hit
          n :: Int
          n = round (x / sx) + round (y / sy)
      in if even n
           then scatter mat1 ray hit
           else scatter mat2 ray hit
    emission = NoEmission

normalShadeMaterial :: Material
normalShadeMaterial =
    Material { scatter = scatter, emission = NoEmission }
  where
    scatter ray hit = do
      let !atten = 0.5 `scaleV` (hitNormal hit `addV` Vec3 1 1 1)
          !direction = hitNormal hit
          !scattered = Ray3 (hitPoint hit) direction
      pure (atten, Just scattered)

-- | Lighter is closer.
distanceShadeMaterial :: Double -> Material
distanceShadeMaterial dist1 =
    Material { scatter = scatter, emission = NoEmission }
  where
    scatter ray hit = do
      let !atten = (hitDistance hit / dist1 - 1) `scaleV` Vec3 1 1 1
          !direction = hitNormal hit
          !scattered = Ray3 (hitPoint hit) direction
      pure (atten, Just scattered)

lambertian :: Vec3          -- ^ albedo
           -> Material
lambertian albedo = Material {..}
  where
    emission = NoEmission
    scatter ray hit
      | direction ray `dotV` hitNormal hit > 0 =
        -- ray originated from inside face
        return (Vec3 1 1 1, Just ray)
      | otherwise = do
        noise <- randomPointInSphere 1
        let !direction = hitNormal hit `addV` noise
            !scattered = Ray3 (hitPoint hit) direction
        return (albedo, Just scattered)

reflect :: Vec3  -- ^ normal
        -> Vec3  -- ^ vector
        -> Vec3  -- ^ reflected
reflect n v = v `subV` ((2 * (v `dotV` n)) `scaleV` n)

metal :: Vec3      -- ^ albedo
      -> Double    -- ^ specularity (0=specular)
      -> Material
metal !albedo !spec0 = Material {..}
  where
    emission = NoEmission
    scatter :: Ray3 -> Hit -> SamplerM s (Vec3, Maybe Ray3)
    scatter ray hit = oneShotState $ do
        let refl = reflect (hitNormal hit) (normaliseV (direction ray))
        direction <- do noise <- randomPointInSphere spec0
                        return $ refl `addV` noise
        let scattered
              | direction `dotV` hitNormal hit > 0 = Just $! Ray3 (hitPoint hit) direction
              | otherwise = Nothing
        return (albedo, scattered)

refract :: Double     -- ^ n / n'
        -> Vec3       -- ^ normal
        -> Vec3       -- ^ vector
        -> Maybe Vec3
refract ni_nt n v
  | discrim > 0 = Just $! (ni_nt `scaleV` term1) `subV` term2
  | otherwise   = Nothing
  where
    uv = normaliseV v
    dt = uv `dotV` n
    discrim = 1 - ni_nt * ni_nt * (1 - dt*dt)
    term1 = uv `subV` (dt `scaleV` n)
    term2 = sqrt discrim `scaleV` n

schlick :: Double -> Double -> Double
schlick cosine refIdx =
    let r0 = (1-refIdx) / (1+refIdx)
        r0' = r0*r0
    in r0' + (1-r0') * (1 - cosine)^(5::Int)

dielectric :: Double -> Material
dielectric !refIdx = Material {..}
  where
    emission = NoEmission
    scatter :: Ray3 -> Hit -> SamplerM s (Vec3, Maybe Ray3)
    scatter ray hit = oneShotState $ do
        scatterDir <-
          case refracted of
            Nothing -> pure reflected
            Just refractDir -> do
              let reflectProb = schlick cosine refIdx
              n <- uniformDouble01M'
              if n < reflectProb
                then pure reflected
                else pure refractDir
        let !scattered = Ray3 (hitPoint hit) scatterDir
        return $! (attenuation, Just scattered)
      where
        attenuation = Vec3 1 1 1

        (out_normal, ni_nt, cosine)
          | direction ray `dotV` hitNormal hit > 0
          = ( negateV (hitNormal hit)
            , refIdx
            , refIdx * alpha
            )
          | otherwise
          = ( hitNormal hit
            , recip refIdx
            , negate alpha
            )
          where
            alpha = (normaliseV (direction ray) `dotV` hitNormal hit)

        refracted = refract ni_nt out_normal (direction ray)
        reflected = reflect (hitNormal hit) (direction ray)
