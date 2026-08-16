{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE ScopedTypeVariables #-}

module Image
  ( Image
  , Coord(..)
    -- * Queries
    -- ** Dimensions
  , imageBounds
  , imageLowerLeft
  , imageUpperRight
    -- ** Pixel data
  , pixels
    -- * Manipulation
  , gammaCorrect
    -- * Creation
  , accumImage
  , generateImage
  , generateImageM
  , blackImage
  , scaleImageValues
  , sumImages
  , averageImages
    -- * I/O
  , imageToPPM
  , writePPMFile
  ) where

import Data.Ix
import Data.Array.Unboxed as A
import Data.Array.MArray as MA
import Data.Array.ST (STUArray)

import Colour
import SamplerMonad

data Coord
  = Coord { coordX, coordY :: !Int }
  deriving (Eq, Ord, Show, Ix)

newtype Image = Image { imageData :: (UArray (Coord, Channel) Double) }

imageBounds :: Image -> (Coord, Coord)
imageBounds img =
  case A.bounds $ imageData img of
    ((a,_), (b,_)) -> (a,b)

imageLowerLeft, imageUpperRight :: Image -> Coord
imageLowerLeft = fst . imageBounds
imageUpperRight = snd . imageBounds

arrayBounds :: (Coord, Coord) -> ((Coord, Channel), (Coord, Channel))
arrayBounds (lowerLeft, upperRight) = ((lowerLeft, Red), (upperRight, Blue))

accumImage :: (Double -> Double -> Double)  -- ^ accumulating function applied channel-wise
           -> (Coord, Coord)                -- ^ coordinate range
           -> [(Coord, Colour)]             -- ^
           -> Image
accumImage f bnds xs =
  Image { imageData = A.accumArray f 0 (arrayBounds bnds)
          [ ((Coord x y, channel), getChannel colour channel)
          | (Coord x y, colour) <- xs
          , channel <- [Red,Green,Blue]
          ]
        }

generateImage :: (Coord, Coord) -> (Coord -> Colour) -> Image
generateImage bnds f =
  accumImage (const id) bnds
  [ (p, f p)
  | p <- range bnds
  ]

generateImageM :: forall s. (Coord, Coord) -> (Coord -> SamplerM s Colour) -> SamplerM s Image
generateImageM bnds gen = do
  arr <- liftST $ MA.newArray_ (arrayBounds bnds)
    :: SamplerM s (STUArray s (Coord, Channel) Double)
  let f :: Coord -> SamplerM s ()
      f coord = do
        colour <- gen coord
        liftST $ do
          MA.writeArray arr (coord, Red)   (cRed colour)
          MA.writeArray arr (coord, Green) (cGreen colour)
          MA.writeArray arr (coord, Blue)  (cBlue colour)

  mapM_ f (range bnds)
  arr' <- liftST $ MA.freeze arr
  return $ Image { imageData = arr' }

blackImage :: (Coord, Coord) -> Image
blackImage bnds = accumImage (const id) bnds []

pixelColour :: Image -> Coord -> Colour
pixelColour (Image{..}) coord =
    Colour { cRed = ch Red, cGreen = ch Green, cBlue = ch Blue }
  where ch c = imageData A.! (coord, c)

pixels :: Image -> [(Coord, Colour)]
pixels img =
  [ (coord, pixelColour img coord)
  | coord <- range (imageBounds img)
  ]

scaleImageValues :: Double -> Image -> Image
scaleImageValues s (Image img) = Image $ A.amap (* s) img

sumImages :: (Coord, Coord) -> [Image] -> Image
sumImages bnds imgs =
    Image $ A.accumArray (+) 0 (arrayBounds bnds) $ foldMap (A.assocs . imageData) imgs

averageImages :: [Image] -> Image
averageImages [] = error "averageImages: empty list"
averageImages (img:rest) =
    img { imageData = A.amap (/ n)
                    $ A.accum (+) (imageData img)
                    $ foldMap (A.assocs . imageData . checkBounds) rest
        }
  where
    n = realToFrac $ length rest + 1
    checkBounds :: Image -> Image
    checkBounds img'
      | imageBounds img /= imageBounds img' = error "averageImages: Incompatible images"
      | otherwise = img'

gammaCorrect :: Double -> Image -> Image
gammaCorrect gamma img = img { imageData = A.amap (**(recip gamma)) (imageData img) }

imageToPPM :: Image -> String
imageToPPM img = unlines
  $ [ "P3"
    , unwords [show width, show height]
    , "255"
    ] <>
    [ unwords [showCh r, showCh b, showCh g]
    | y <- [y1-1, y1-2.. y0]
    , x <- [x0..x1-1]
    , let Colour r b g = pixelColour img (Coord x y)
    ]
  where
    (Coord x0 y0, Coord x1 y1) = imageBounds img
    width = x1 - x0
    height = y1 - y0
    showCh = show . f
      where
        f :: Double -> Int
        f = round . min 255 . max 0 . (255 *)

writePPMFile :: FilePath -> Image -> IO ()
writePPMFile fpath img = writeFile fpath $ imageToPPM img
