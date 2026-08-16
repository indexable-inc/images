module Scene.Scene1 (mkScene, camera) where

import Control.Monad
import Control.Monad.Trans.State.Strict
import System.Random.Stateful

import Colour
import Figure
import Matrix
import Mesh
import RandomDist (choose)
import Sampler
import Scene.Type
import STL
import Vector

mkScene :: IO Scene
mkScene = do
  figures <- monkey
  return $ Scene {sceneFigures = figures, sceneCamera = camera, sceneWorld = const $ Colour 0.5 0.5 0.5}

camera :: Camera
camera = mkCamera lookFrom lookAt viewUp fov aspect aperture
  where
    lookFrom = Pt $ Vec3 0 0 5
    lookAt   = Pt $ Vec3 0 0 (-1)
    viewUp   = Vec3 0 (-1) 0
    fov      = 45
    aspect   = 2
    aperture = PointAperture

meshToFigures :: Material -> Mesh.Mesh -> [Figure]
meshToFigures material = map convertTri . Mesh.meshTriangles
  where
    convertTri (Mesh.Triangle a b c) = triangle a b c material

monkey :: IO [Figure]
monkey = do
    mesh <- rotateMesh (rotationMat3 (Vec3 1 0 0) pi) <$> STL.readSTL "monkey.stl"
    mesh2 <- rotateMesh (rotationMat3 (Vec3 1 0 0) pi) <$> STL.readSTL "teapot.stl"
    let figs = concat
               [ meshToFigures material1 $ translateMesh (Vec3 (-2) 0 0) mesh
               , meshToFigures material2 $ translateMesh (Vec3 2    0 (-1)) mesh2
               , meshToFigures floorMaterial 
                 $ translateMesh (Vec3 0 1 (-1))
                 $ rotateMesh (rotationMat3 (Vec3 1 0 0) (pi/2))
                 $ xyQuadMesh (Vec2 5 5)
               , [sphere (Pt $ Vec3 0 0.5 2.5) 0.3 (dielectric 2.2)]
               , manySpheres
               ]
    return $ figs
  where
    material1 = withConstEmission (Colour 0.3 0 0) $ lambertian (Vec3 0.6 0.4 0.4)
    material2 = metal (Vec3 0.6 0.6 0.6) 0.2
    floorMaterial = metal (Vec3 0.3 0.3 0.4) 0.3
    grayMat n = lambertian (Vec3 n n n)

threeSpheres :: [Figure]
threeSpheres =
  [ sphere (Pt $ Vec3 0    0        (-1))   0.5             (lambertian $ Vec3 0.7 0 0)
  , sphere (Pt $ Vec3 1    0        (-1))   0.5             (metal (Vec3 0.8 0.6 0.2) 0.1)
  , sphere (Pt $ Vec3 (-1) 0        (-1))   0.5             (dielectric 2.2)
  , sphere (Pt $ Vec3 (-1) 1        (-1))   0.2             (withConstEmission (Colour 0.8 0.8 0.2) $ lambertian $ Vec3 0.8 0.8 0.2)
  , sphere (Pt $ Vec3 0    (-100.5) (-1))   100             (lambertian $ Vec3 0.2 0.2 0.8)
  ]

manySpheres :: [Figure]
manySpheres =
    spheres -- ++ [ floor ]
  where
    spheres =
      runStateGen_ (mkStdGen 42) $ replicateM 150 . sampleSphere

    floor =
      triangle p0 p1 p2
      $ metal (Vec3 0.7 0.7 0.7) 0.2
      where
        p0 = Pt $ Vec3 0     y 0
        p1 = Pt $ Vec3 10    y (-10)
        p2 = Pt $ Vec3 (-10) (y-1) (-10)
        y = (-3)

    sampleSphere :: RandomGen g => StateGenM g -> State g Figure
    sampleSphere gen = do
        pos <- Vec3 <$> d (-10,10) <*> d (-4,1) <*> d (0,-10)
        rad <- d (0.1, 0.5)
        colour <- Vec3 <$> d (0,1) <*> d (0,1) <*> d (0,1)
        material <- choose
          [ (1, return $ metal colour 0.4)
          , (0.3, return $ dielectric 2.0)
          ] StateGenM
        return $ sphere (Pt pos) rad material
      where
        d (a,b) = uniformRM (a,b) gen
