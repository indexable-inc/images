module Scene.Scene2 (mkScene) where

import Colour
import Figure
import Sampler
import Scene.Type
import Vector

pt3 x y z = Pt $ Vec3 x y z

inv :: Vec3 -> Vec3
inv x = Vec3 1 1 1 `subV` x

mkScene :: IO Scene
mkScene = pure $ Scene { sceneFigures = figures, sceneCamera = camera, sceneWorld = const $ Colour 0.5 0 0  }

figures :: [Figure]
figures =
  [ sphere (pt3 (-1e5-1)  40.8         81.6)        1e5      (lambertian $ Vec3 0.75 0.25 0.25)       -- left
  , sphere (pt3 ( 1e5+99) 40.8         81.6)        1e5      (lambertian $ Vec3 0.25 0.25 0.75)       -- right
  , sphere (pt3 50        40.8         (-1e5))      1e5      (lambertian $ Vec3 0.75 0.75 0.75)       -- back
  , sphere (pt3 50        40.8         (-1e5+201))  1e5      (lambertian $ Vec3 0 0 0)                -- front
  , sphere (pt3 50        (-1e5)       81.6)        1e5      (lambertian $ Vec3 0.75 0.75 0.75)       -- bottom
  , sphere (pt3 50        (-1e5+81.7)  81.6)        1e5      (lambertian $ Vec3 0.75 0.75 0.75)       -- top (reflect up)
  , sphere (pt3 50        ( 1e5+81.6)  81.6)        1e5      (lambertian $ Vec3 0.75 0.75 0.75)       -- top (reflect down)
  , sphere (pt3 27        16.5         47)          16.5     (metal (0.999 `scaleV` Vec3 1 1 1) 0)    -- mirror
  , sphere (pt3 73        16.5         78)          16.5     (dielectric 1.5)                         -- glass
  , sphere (pt3 50        (681.6-0.27) 81.6)        600      (withConstEmission (Colour 12 12 12) $ lambertian $ Vec3 0 0 0)  -- lamp
  ]

camera :: Camera
camera = mkCamera lookFrom lookAt viewUp fov aspect aperture
  where
    lookFrom = Pt $ Vec3 50 52 200
    lookAt   = lookFrom `addP` Vec3 0 (-0.042612) (-1)
    --lookAt   = pt3 27 16.5 47
    viewUp   = Vec3 0 1 0
    fov      = 60
    aspect   = 4/3
    aperture = PointAperture
