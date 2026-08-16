module Scene.Type (Scene(..)) where

import Figure
import Sampler

data Scene
  = Scene { sceneFigures :: [Figure] 
          , sceneCamera  :: Camera
          , sceneWorld   :: World
          }
