{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE PatternSynonyms #-}
{-# LANGUAGE TupleSections #-}
{-# LANGUAGE ViewPatterns #-}
module Main (main) where
import Control.Monad
import Control.Monad.ST
import Data.List(foldl')
import Data.Bits
import Data.STRef.Unboxed
import Data.Word
import GHC.Float (castWord64ToDouble)
import Text.Printf
import System.IO (Handle, stdout, hPutStr)
import System.Environment (getArgs)
-- position, also color (r,g,b)
data Vec = Vec {-# UNPACK #-} !Double {-# UNPACK #-} !Double {-# UNPACK #-} !Double

zerov :: Vec
zerov = Vec 0 0 0
addv, subv, mulv, cross :: Vec -> Vec -> Vec
addv (Vec a b c) (Vec x y z) = Vec (a+x) (b+y) (c+z)
subv (Vec a b c) (Vec x y z) = Vec (a-x) (b-y) (c-z)
mulv (Vec a b c) (Vec x y z) = Vec (a*x) (b*y) (c*z)
cross (Vec a b c) (Vec x y z) = Vec (b*z-c*y) (c*x-a*z) (a*y-b*x)
mulvs :: Vec -> Double -> Vec
mulvs (Vec a b c) x = Vec (a*x) (b*x) (c*x)
len :: Vec -> Double
len (Vec a b c) = sqrt $ a*a+b*b+c*c
norm :: Vec -> Vec
norm v = v `mulvs` (1/len v)
dot :: Vec -> Vec -> Double
dot (Vec a b c) (Vec x y z) = a*x+b*y+c*z
maxv :: Vec -> Double
maxv (Vec a b c) = max a (max b c)

data Ray = Ray {-# UNPACK #-} !Vec {-# UNPACK #-} !Vec -- origin, direction

newtype Refl = Refl Int  -- material types, used in radiance
pattern DIFF,SPEC,REFR :: Refl
pattern DIFF = Refl 0
pattern SPEC = Refl 1
pattern REFR = Refl 2
{-# COMPLETE DIFF, SPEC, REFR #-}

-- radius, position, emission, color, reflection
data Sphere = Sphere {-# UNPACK #-} !Double {-# UNPACK #-} !Vec {-# UNPACK #-} !Vec {-# UNPACK #-} !Vec {-# UNPACK #-} !Refl

sphLeft, sphRight, sphBack, sphFrnt, sphBotm, sphTop, sphMirr, sphGlas, sphLite :: Sphere
sphLeft  = Sphere 1e5  (Vec (1e5+1) 40.8 81.6)    zerov (Vec 0.75 0.25 0.25) DIFF --Left
sphRight = Sphere 1e5  (Vec (-1e5+99) 40.8 81.6)  zerov (Vec 0.25 0.25 0.75) DIFF --Rght
sphBack  = Sphere 1e5  (Vec 50 40.8 1e5)          zerov (Vec 0.75 0.75 0.75) DIFF --Back
sphFrnt  = Sphere 1e5  (Vec 50 40.8 (-1e5+170))   zerov zerov              DIFF --Frnt
sphBotm  = Sphere 1e5  (Vec 50 1e5 81.6)          zerov (Vec 0.75 0.75 0.75) DIFF --Botm
sphTop   = Sphere 1e5  (Vec 50 (-1e5+81.6) 81.6)  zerov (Vec 0.75 0.75 0.75) DIFF --Top
sphMirr  = Sphere 16.5 (Vec 27 16.5 47)           zerov (Vec 0.999 0.999 0.999) SPEC --Mirr
sphGlas  = Sphere 16.5 (Vec 73 16.5 78)           zerov (Vec 0.999 0.999 0.999) REFR --Glas
sphLite  = Sphere 600  (Vec 50 (681.6-0.27) 81.6) (Vec 12 12 12)       zerov DIFF --Lite

intersect :: Ray -> Sphere -> Double
intersect (Ray o d) (Sphere r p _e _c _refl) =
  if det<0 then (1/0.0) else f (b-sdet) (b+sdet)
  where op = p `subv` o
        eps = 1e-4
        b = op `dot` d
        det = b*b - (op `dot` op) + r*r
        sdet = sqrt det
        f a s = if a>eps then a else if s>eps then s else (1/0.0)

clamp :: (Ord p, Num p) => p -> p
clamp x = if x<0 then 0 else if x>1 then 1 else x

toInt :: Double -> Int
toInt x = floor $ clamp x ** (1/2.2) * 255 + 0.5

intersects :: Ray -> (Double, Sphere)
intersects ray =
    f (f (f (f (f (f (f (f (intersect ray sphLeft, sphLeft) sphRight) sphBack) sphFrnt) sphBotm) sphTop) sphMirr) sphGlas) sphLite
  where
    f !(!k', !sp) !s' = let !x = intersect ray s' in if x < k' then (x, s') else (k', sp)

radiance :: Ray -> Int -> STRefU s Word64 -> ST s Vec
radiance ray@(Ray o d) depth xi = case intersects ray of
  (t,_) | t == (1/0.0) -> return zerov
  (t,Sphere _r p e c refl) -> do
    let x = o `addv` (d `mulvs` t)
        n = norm $ x `subv` p
        nl = if n `dot` d < 0 then n else n `mulvs` (-1)
        depth' = depth + 1
        continue f = case refl of
          DIFF -> do
            r1 <- ((2*pi)*) `fmap` erand48 xi
            r2 <- erand48 xi
            let r2s = sqrt r2
                w@(Vec wx _ _) = nl
                u = norm $ (if abs wx > 0.1 then (Vec 0 1 0) else (Vec 1 0 0)) `cross` w
                v = w `cross` u
                d' = norm $ (u`mulvs`(cos r1*r2s)) `addv` (v`mulvs`(sin r1*r2s)) `addv` (w`mulvs`sqrt (1-r2))
            rad <- radiance (Ray x d') depth' xi
            return $ e `addv` (f `mulv` rad)

          SPEC -> do
            let d' = d `subv` (n `mulvs` (2 * (n`dot`d)))
            rad <- radiance (Ray x d') depth' xi
            return $ e `addv` (f `mulv` rad)

          REFR -> do
            let reflRay = Ray x (d `subv` (n `mulvs` (2* n`dot`d))) -- Ideal dielectric REFRACTION
                into = n`dot`nl > 0                -- Ray from outside going in?
                nc = 1
                nt = 1.5
                nnt = if into then nc/nt else nt/nc
                ddn= d`dot`nl
                cos2t = 1-nnt*nnt*(1-ddn*ddn)
            if cos2t<0    -- Total internal reflection
              then do
                rad <- radiance reflRay depth' xi
                return $ e `addv` (f `mulv` rad)
              else do
                let tdir = norm $ (d`mulvs`nnt `subv` (n`mulvs`((if into then 1 else -1)*(ddn*nnt+sqrt cos2t))))
                    a=nt-nc
                    b=nt+nc
                    r0=a*a/(b*b)
                    c' = 1-(if into then -ddn else tdir`dot`n)
                    re=r0+(1-r0)*c'*c'*c'*c'*c'
                    tr=1-re
                    pp=0.25+0.5*re
                    rp=re/pp
                    tp=tr/(1-pp)
                rad <-
                  if depth>2
                    then do er <- erand48 xi
                            if er<pp -- Russian roulette
                              then (`mulvs` rp) `fmap` radiance reflRay depth' xi
                              else (`mulvs` tp) `fmap` radiance (Ray x tdir) depth' xi
                    else do rad0 <- (`mulvs` re) `fmap` radiance reflRay depth' xi
                            rad1 <- (`mulvs` tr) `fmap` radiance(Ray x tdir) depth' xi
                            return $ rad0 `addv` rad1
                return $ e `addv` (f `mulv` rad)

    if depth'>5
      then do
        er <- erand48 xi
        let !pr = maxv c
        if er < pr then continue $ c `mulvs` (1/pr)
                  else return e
      else continue c

smallpt :: Int -> Int -> Int -> IO [Vec]
smallpt w h nsamps = do
  let samps = nsamps
      org = Vec 50 52 295.6
      dir = norm $ Vec 0 (-0.042612) (-1)
      cx = Vec (fromIntegral w * 0.5135 / fromIntegral h) 0 0
      cy = norm (cx `cross` dir) `mulvs` 0.5135
      img = (`concatMap` [(h-1),(h-2)..0]) $ \y -> runST $ do
        xi <- newSTRefU (mkErand48Seed' y)
        forM [0..w-1] $ \x -> do
          (\pf -> foldM pf zerov [(sy, sx) | sy <- [0,1], sx <- [0,1]]) $ \ci (sy, sx) -> do
            Vec rr rg rb <- (\f -> foldM f zerov [0..samps-1]) $ \ !r _s -> do
              r1 <- (2*) `fmap` erand48 xi
              let !dx = if r1<1 then sqrt r1-1 else 1-sqrt(2-r1)
              r2 <- (2*) `fmap` erand48 xi
              let !dy = if r2<1 then sqrt r2-1 else 1-sqrt(2-r2)
                  !d = (cx `mulvs` (((sx + 0.5 + dx)/2 + fromIntegral x)/fromIntegral w - 0.5)) `addv`
                       (cy `mulvs` (((sy + 0.5 + dy)/2 + fromIntegral y)/fromIntegral h - 0.5)) `addv` dir
              rad <- radiance (Ray (org`addv`(d`mulvs`140)) (norm d)) 0 xi
              -- Camera rays are pushed forward ^^^^^ to start in interior
              pure $ (r `addv` (rad `mulvs` (1 / fromIntegral samps)))
            pure $ ci `addv` (Vec (clamp rr) (clamp rg) (clamp rb) `mulvs` 0.25)

  return img

writeImage :: Handle -> (Int, Int) -> [Vec] -> IO ()
writeImage hdl (w,h) img = do
  hPrintf hdl "P3\n%d %d\n%d\n" w h (255::Int)
  forM_ img $ \(Vec r g b) -> do
    hPrintf hdl "%d %d %d " (toInt r) (toInt g) (toInt b)

mkErand48Seed' :: Int -> Word64
mkErand48Seed' y =
  let yw = fromIntegral y; prod = (yw * yw * yw)
  in concatWord 0 0 prod

concatWord :: Word64 -> Word64 -> Word64 -> Word64
concatWord x0 x1 x2 = x0 .|. (x1 `unsafeShiftL` 16) .|. (x2 `unsafeShiftL` 32)

-- | returns the full state and the bitmasked value.
rand48_step :: Word64 -> (Word64, Word64)
rand48_step x =
  let a = concatWord lc0 lc1 lc2
      x' = (a * x) + lc3
      lc0 = 0xe66d
      lc1 = 0xdeec
      lc2 = 0x5
      lc3 = 0xb
  in (x', x' .&. 0xffffffffffff)

erand48' :: Word64 -> (Word64, Double)
erand48' s =
  let (s', out') = (rand48_step s)
      d_word = (0x3ff0000000000000) .|. (out' `unsafeShiftL` 4)
  in (s' , (castWord64ToDouble d_word) - 1.0)

erand48 :: STRefU s Word64 -> ST s Double
erand48 !t =  do
  r <- readSTRefU t
  let (r', d) = erand48' r
  writeSTRefU t r'
  pure d

hashImage :: [Vec] -> String
hashImage vecs = show $ foldl' (\n (Vec a b c) -> n + a + b + c) 0 vecs

-- Must be called as ./Main img/hash <n>
main :: IO ()
main = do
  [mode, read -> samples] <- getArgs
  let n = 512
      w = n
      h = (round $ realToFrac n/1.3333)
  vec <- smallpt w h samples
  if mode == "img"
    then writeImage stdout (w, h) vec
    else hPutStr stdout (printf "%.6f" (read (hashImage vec) :: Double))
