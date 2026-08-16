{-# LANGUAGE DeriveFunctor #-}
{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE GeneralizedNewtypeDeriving #-}
{-# LANGUAGE OverloadedStrings #-}

module Measurements
    ( -- * Labels
      -- | A label hierarchically identifies a type of measurement (e.g.
      -- @T1234//bytes allocated@)
      Label(..)
    , mkLabel
    , parseLabel
    , encodeLabel

      -- * Measurements
    , Measurements(..)
    , toMap
    , fromMap
    , prefix
    , singleton
    , fromList
    , filterByLabel
    , mapLabels
      -- ** I/O with JSON representation
    , writeFile'
    , writeFile
    , readFileJson
    , readFileTsv
    , readFile
      -- ** I/O with CSV representation
    , writeFileTsv

      -- * Miscellaneous utilities
    , mean, geomMean, stdDev
    ) where

import Data.Word (Word8)
import Data.String (IsString(..))
import Data.Bifunctor
import Data.Maybe
import Control.Applicative
import Control.Monad
import Data.Aeson
import qualified Data.Csv as Csv
import qualified Data.Vector as V
import qualified Data.Map.Strict as M
import qualified Data.ByteString.Lazy.Char8 as BSL
import qualified Data.Text as T
import System.FilePath
import Prelude hiding (readFile, writeFile)

newtype Label = Label { getLabel :: [T.Text] }
              deriving (Eq, Ord, Show, Monoid, Semigroup,
                        ToJSON, FromJSON, ToJSONKey, FromJSONKey)

instance IsString Label where
    fromString s = Label [fromString s]

instance Csv.ToField Label where
    toField = Csv.toField . encodeLabel

instance Csv.FromField Label where
    parseField = pure . parseLabel <=< Csv.parseField

mkLabel :: String -> Label
mkLabel = fromString

parseLabel :: T.Text -> Label
parseLabel = Label . T.splitOn "//"

encodeLabel :: Label -> T.Text
encodeLabel (Label parts) = T.intercalate "//" parts

newtype Measurements a = Measurements { toList :: [(Label, a)] }
    deriving (Show, Functor, Monoid, Semigroup, ToJSON, FromJSON)

toMap :: ([a] -> b) -> Measurements a -> M.Map Label b
toMap f (Measurements xs) = 
    fmap f $ M.fromListWith (<>) [ (x, [y]) | (x,y) <- xs ]

fromMap :: M.Map Label a -> Measurements a
fromMap = Measurements . M.toList

prefix :: Label -> Measurements a -> Measurements a
prefix lbl (Measurements xs) = Measurements $ map (first (lbl<>)) xs

singleton :: Label -> a -> Measurements a
singleton lbl x = Measurements [(lbl, x)]

fromList :: [(Label, a)] -> Measurements a
fromList = Measurements

filterByLabel :: (Label -> Bool) -> Measurements a -> Measurements a
filterByLabel f (Measurements xs) = Measurements $ filter (f . fst) xs

mapLabels :: (Label -> Maybe Label) -> Measurements a -> Measurements a
mapLabels f (Measurements xs) =
    Measurements $ mapMaybe (\(k,v) -> (\k' -> (k', v)) <$> f k) xs

writeFile' :: ToJSON a => FilePath -> Measurements a -> IO ()
writeFile' fname = BSL.writeFile fname . encode

writeFile :: FilePath -> Measurements Double -> IO ()
writeFile = writeFile'

readFileJson :: FromJSON a => FilePath -> IO (Measurements a)
readFileJson fname = eitherDecodeFileStrict fname >>= either fail pure

readFileTsv :: FilePath -> IO (Measurements Double)
readFileTsv path = either err f . Csv.decodeWith opts Csv.NoHeader <$> BSL.readFile path
  where
    f = Measurements . map (first parseLabel) . V.toList
    err e = error $ path ++ ": failed to read TSV: " ++ show e
    opts :: Csv.DecodeOptions
    opts = Csv.defaultDecodeOptions { Csv.decDelimiter = tabChar }

tabChar :: Word8
tabChar = 9

readFile :: FilePath -> IO (Measurements Double)
readFile fname
  | "json" <- takeExtension fname = readFileJson fname
  | otherwise                     = readFileTsv fname

writeFileTsv :: FilePath -> Measurements Double -> IO ()
writeFileTsv fname = BSL.writeFile fname . Csv.encodeByNameWith opts header . map f . toList
  where
    f (label, value) = Csv.namedRecord [ "label" Csv..= label, "value" Csv..= value ]
    header = V.fromList ["label", "value"]
    opts :: Csv.EncodeOptions
    opts = Csv.defaultEncodeOptions { Csv.encDelimiter = tabChar
                                    , Csv.encIncludeHeader = False
                                    , Csv.encQuoting = Csv.QuoteNone
                                    }

mean :: RealFrac a => [a] -> a
mean xs = sum xs / realToFrac (length xs)

geomMean :: RealFloat a => [a] -> a
geomMean = exp . mean . map log

stdDev :: RealFloat a => [a] -> a
stdDev xs = sqrt $ mean $ map (\x -> (x-m)^(2::Int)) xs
  where m = mean xs

