{-# LANGUAGE CPP #-}
{-# LANGUAGE GeneralizedNewtypeDeriving #-}
{-# LANGUAGE RankNTypes #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE BangPatterns #-}

module RandomDist
  ( randomPointInSphere
  , randomPointInDisk
  , uniformDouble01M'
  , choose
  ) where

import System.Random.Stateful (StatefulGen, uniformDouble01M, uniformM)

#if defined(RNG_SIGNED)
import GHC.Int
#endif

#if defined(RNG_NAIVE)
import GHC.Word (Word64)
#endif

import SamplerMonad
import Vector

randomPointInDisk :: Double -> SamplerM s Vec2
randomPointInDisk radius = do
  r <- (radius *) <$> uniformDouble01M'
  theta <- ((2*pi) *) <$> uniformDouble01M'
  let x = r * cos theta
  let y = r * sin theta
  return $! Vec2 x y

randomPointInSphere :: Double  -- ^ radius
                    -> SamplerM s Vec3
randomPointInSphere = randomPointInSphereRej

randomPointInSpherePolar :: Double -> SamplerM s Vec3
randomPointInSpherePolar !radius = do
    u <- (\x -> 2*x - 1) <$> uniformDouble01M'
    phi <- ((2*pi) *) <$> uniformDouble01M'
    r <- (radius *) . (**(1/3)) <$> uniformDouble01M'
    let x = r * cos phi * sqrt (1 - u*u)
    let y = r * sin phi * sqrt (1 - u*u)
    let z = r * u
    return $! Vec3 x y z

randomPointInSphereRej :: Double -> SamplerM s Vec3
randomPointInSphereRej !radius = go
  where
    f = uniformDouble01M'
    go = do
      x <- Vec3 <$> f <*> f <*> f
      let x' = (2 `scaleV` x) `subV` Vec3 1 1 1
      if quadranceV x' >= 1
        then go
        else return $! radius `scaleV` x'

uniformDouble01M' :: forall s. SamplerM s Double
#if defined(RNG_NAIVE)
uniformDouble01M' = oneShotState $ do
  w64 <- uniformWord64 gen :: SamplerM s Word64
  return $ fromIntegral w64 / m
  where m = fromIntegral (maxBound :: Word64) :: Double
#elif defined(RNG_SIGNED)
uniformDouble01M' = oneShotState $ do
  i64 <- uniformM gen :: SamplerM s Int64
  return $ fromIntegral (abs i64) / m
  where m = fromIntegral (maxBound :: Int64) :: Double
#else
uniformDouble01M' = uniformDouble01M gen
#endif
{-# INLINE uniformDouble01M' #-}

choose :: forall g m a. StatefulGen g m => [(Double, m a)] -> g -> m a
choose xs gen = do
    p <- uniformDouble01M gen
    go (mass * p) xs
  where
    mass = sum $ map fst xs

    go :: Double -> [(Double, m a)] -> m a
    go !_ [(_, action)] = action
    go p  ((q, action) : rest)
      | q > p = action
      | otherwise = go (p-q) rest
    go !_ [] = error "choose: no options from which to choose"
