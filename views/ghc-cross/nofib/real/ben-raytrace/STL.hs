{-# LANGUAGE RecordWildCards #-}

module STL
  ( Triangle(..)
  , readSTL
  ) where

import Control.Monad

import qualified Data.ByteString.Lazy as BSL
import Data.Binary.Get

import Vector
import Mesh

readSTL :: FilePath -> IO Mesh
readSTL fpath =
    Mesh . runGet getSTL <$> BSL.readFile fpath

getSTL :: Get [Triangle]
getSTL = do
    skip 80
    n <- getWord32le
    replicateM (fromIntegral n) $ do
        _normal <- getVector
        triA <- Pt <$> getVector
        triB <- Pt <$> getVector
        triC <- Pt <$> getVector
        attrBytes <- getWord16le
        skip $ fromIntegral attrBytes
        return $! Triangle {..}

getVector :: Get Vec3
getVector =
    Vec3 <$> getFloat
         <*> getFloat
         <*> getFloat
  where
    getFloat = realToFrac <$> getFloatle