{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE NamedFieldPuns #-}
{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE RankNTypes #-}

module Sampler
    ( Camera(..)
    , Aperture(..)
    , mkCamera, mkCameraRaw
    , cameraAspect
    , Sampler
    , World
    , sampleImagePixel
    , supersampleImagePixel
    ) where

import Colour
import Figure
import Image (Coord(..))
import Interval
import RandomDist
import Ray
import SamplerMonad
import Vector

-- | After how many bounces to forcibly truncate transport?
mAX_BOUNCES :: Int
mAX_BOUNCES = 50

-- | After how many bounces to start truncating transport?
rOULETTE_BOUNCES :: Int
rOULETTE_BOUNCES = 5

tOO_MANY_BOUNCES_COLOUR :: Colour
tOO_MANY_BOUNCES_COLOUR = black

data Aperture
  = PointAperture
  | FiniteAperture { focalLength  :: !Double
                   , apertureDiam :: !Double
                   }

data Camera
  = Camera { lowerLeft  :: !(Pt Vec3)
           , horizontal :: !Vec3
           , vertical   :: !Vec3
           , cameraU, cameraV, cameraW :: !Vec3
           , rayOrigin  :: !(Pt Vec3)
           , aperture   :: !Aperture
           }

mkCamera
    :: Pt Vec3  -- ^ Look-from
    -> Pt Vec3  -- ^ Look-at
    -> Vec3     -- ^ View-up
    -> Double   -- ^ Field-of-view (degrees)
    -> Double   -- ^ Aspect ratio
    -> Aperture
    -> Camera
mkCamera lookFrom lookAt viewUp fov aspect aperture =
    Camera {..}
  where
    theta = fov / 180 * pi
    h = tan (theta / 2)
    viewport_h = 2 * h
    viewport_w = viewport_h * aspect

    w = normaliseV (lookFrom `subP` lookAt)
    u = normaliseV (viewUp `crossV3` w)
    v = w `crossV3` u

    focal = case aperture of
              PointAperture -> 1
              FiniteAperture {focalLength} -> focalLength

    rayOrigin  = lookFrom
    horizontal = (focal * viewport_w) `scaleV` u
    vertical   = (focal * viewport_h) `scaleV` v
    lowerLeft  = rayOrigin `addP` negateV (scaleV 0.5 horizontal `addV` scaleV 0.5 vertical `addV` scaleV focal w)
    cameraU    = u
    cameraV    = v
    cameraW    = w

cameraAspect :: Camera -> Double
cameraAspect (Camera{..}) =
    normV horizontal / normV vertical

mkCameraRaw
    :: Pt Vec3  -- ^ Lower-left corner
    -> Double   -- ^ Horizontal scale
    -> Double   -- ^ Vertical scale
    -> Aperture
    -> Camera
mkCameraRaw lowerLeft h v aperture =
  Camera { lowerLeft
         , horizontal = Vec3 h 0 0
         , vertical   = Vec3 0 v 0
         , cameraU    = Vec3 1 0 0
         , cameraV    = Vec3 0 1 0
         , cameraW    = Vec3 0 0 1
         , rayOrigin  = originP
         , aperture   = aperture
         }

-- | Turn a pixel coordinate into a point in image-space.
pixelToImage :: Coord -> Pt Vec2
pixelToImage (Coord i j) = Pt $ Vec2 (realToFrac i) (realToFrac j)

imageToScreenSpace :: Coord -> Pt Vec2 -> Pt Vec2
imageToScreenSpace size (Pt (Vec2 u v)) = Pt $ Vec2 u' v'
  where
    u' = u / realToFrac (coordX size)
    v' = v / realToFrac (coordY size)

cameraRay
    :: Camera
    -> Pt Vec2  -- ^ Point to map
    -> SamplerM s Ray3
cameraRay (Camera{..}) (Pt (Vec2 u v)) =
  case aperture of
    PointAperture  -> return $! Ray3 rayOrigin (direction `subP` rayOrigin)
    FiniteAperture{apertureDiam}  -> do
      let lens_radius = apertureDiam / 2
      Vec2 rx ry <- randomPointInDisk lens_radius
      let offset = rx `scaleV` cameraU `addV` ry `scaleV` cameraV
      return $! Ray3 rayOrigin (direction `subP` rayOrigin `subV` offset)
  where
    direction = lowerLeft `addP` (u `scaleV` horizontal) `addP` (v `scaleV` vertical)

type Sampler s
    =  Coord    -- ^ image size
    -> Camera   -- ^ camera
    -> World    -- ^ external emissions
    -> Figure   -- ^ objects
    -> Coord    -- ^ sampled point
    -> SamplerM s Colour

-- | Emissions from the outside world.
type World = Ray3 -> Colour

sampleImagePixel :: Sampler s
sampleImagePixel size camera world figure coord = do
    ray <- cameraRay camera
      $ imageToScreenSpace size
      $ pixelToImage coord
    sample world mAX_BOUNCES figure ray

revReplicateM :: Monad m => Int -> m a -> m [a]
revReplicateM n0 action = go n0 []
  where
    go 0 accum = return accum
    go n0 accum = do
      x <- action
      go (n0-1) (x:accum)

supersampleImagePixel
    :: forall s. ()
    => Int         -- ^ sample count
    -> Sampler s
supersampleImagePixel nSamples size camera world figure = \coord -> do
    let !x0 = pixelToImage coord
    let drawSample = do
          offset <- sampleOffset
          ray <- cameraRay camera
                   $  imageToScreenSpace size
                   $  x0 `addP` offset
          sample world mAX_BOUNCES figure ray
    samples <- revReplicateM nSamples drawSample
    return $! averageColours samples
  where
    sampleOffset :: SamplerM s Vec2
    sampleOffset = Vec2 <$> m <*> m
      where m = uniformDouble01M'

sample :: World -> Int ->  Figure -> Ray3 -> SamplerM s Colour
sample _world (-1) figure !ray = return tOO_MANY_BOUNCES_COLOUR
sample world bounceLimit figure !ray = oneShotState $ do
  case hitTest figure ray (Interval 1e-4 infinity) of
    Nothing  -> pure $ world ray
    --Just hit -> hitColour hit
    --Just hit -> pure $ normalToColour (hitNormal hit)
    Just hit -> do
      let emitted = emission (hitMaterial hit)
      (atten, mb_scattered) <- scatter (hitMaterial hit) ray hit
      case mb_scattered of
        Nothing -> pure $! addEmission emitted black
        Just scattered
          | bounceLimit < (mAX_BOUNCES - rOULETTE_BOUNCES) -> do
            let maxAtten = vec3X atten `max` vec3Y atten `max` vec3Z atten
            r <- uniformDouble01M'
            if maxAtten > r
              then do
                color <- sample world (bounceLimit-1) figure scattered
                return $! addEmission emitted $ attenuate atten color
              else return $! addEmission emitted black
          | otherwise -> do
            color <- sample world (bounceLimit-1) figure scattered
            return $! addEmission emitted $ attenuate atten color
  where
    infinity = 1/0

normalToColour :: Vec3 -> Colour
normalToColour v = Colour r g b
  where
    Vec3 r g b = 0.5 `scaleV` (v `addV` Vec3 1 1 1)
