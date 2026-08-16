{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE NamedFieldPuns #-}
{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE ScopedTypeVariables #-}

import Control.Parallel.Strategies

import qualified BVH
import qualified UnboxedBVH as UBVH
import Figure
import Image
import Sampler
import SamplerMonad
import Scene.Type
import Scene.Scene2 as Scene
import System.Environment

partitionPlane :: Coord -> Int -> [(Coord, Coord)]
partitionPlane size chunkSize =
    [ (lowerLeft, upperRight)
    | i <- [0 .. (coordX size + chunkSize - 1) `div` chunkSize - 1]
    , j <- [0 .. (coordY size + chunkSize - 1) `div` chunkSize - 1]
    , let lowerLeft = Coord (chunkSize * i) (chunkSize * j)
          upperRight = Coord (min (coordX size) (coordX lowerLeft + chunkSize - 1))
                             (min (coordY size) (coordY lowerLeft + chunkSize - 1))
    ]

main :: IO ()
main = do
    scene <- mkScene
    [size_str] <- getArgs

    let nRuns = 1 :: Int
        chunkSize = 64 :: Int
        nSamples = 32  :: Int
        size = let n = read size_str
                   aspect = cameraAspect $ sceneCamera scene
               in Coord n (round $ realToFrac n / aspect)
        nRays = nRuns * nSamples * coordX size * coordY size

    putStrLn $ "Rays: " ++ show nRays
    putStrLn $ "Figures: " ++ show (length $ sceneFigures scene)

    --let !figure = sampleBVHFigure $ sceneFigures scene
    let figure = mconcat $ sceneFigures scene

    putStrLn $ "Sampling..."
    -- let !sampler = sampleImagePixel size (sceneCamera scene) figure
    let !sampler = supersampleImagePixel nSamples size (sceneCamera scene) (sceneWorld scene) figure

    let singleImg :: Int -> [Image]
        singleImg seed =
          [ runSamplerM (fromIntegral seed) (generateImageM chunk sampler)
          | chunk <- imgChunks
          ]
        imgChunks :: [(Coord, Coord)]
        imgChunks = partitionPlane size chunkSize

    let !img = scaleImageValues (recip $ realToFrac nRuns)
              $ sumImages (Coord 0 0, size)
              $ withStrategy (parBuffer 1024 rseq)
              $ foldMap singleImg [1..nRuns]

    writePPMFile "out.ppm" $ gammaCorrect 2 img
    return ()

sampleBVHFigure :: [Figure] -> Figure
sampleBVHFigure figures =
    UBVH.bvhFigure $ UBVH.packBVH $ runSamplerM 42 (BVH.mkBVH figures)
    --BVH.bvhFigure $ runSamplerM 42 (BVH.mkBVH figures)

