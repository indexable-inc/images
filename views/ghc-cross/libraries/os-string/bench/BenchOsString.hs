{-# LANGUAGE CPP #-}
{-# LANGUAGE BangPatterns        #-}
{-# LANGUAGE PackageImports      #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE MagicHash           #-}
{-# LANGUAGE OverloadedStrings   #-}
{-# LANGUAGE QuasiQuotes         #-}
{-# LANGUAGE TypeApplications    #-}

#define OSSTR osstr
#define OS_STRING OsString
#define OS_CHAR OsChar


module BenchOsString (benchMark) where

import           Data.Type.Coercion                    (coerceWith, sym)
import           System.OsString                       (osstr)
import qualified System.OsString                       as S
import           System.OsString.Internal.Types        (OsString(..), OsChar(..), PosixChar(..), WindowsChar(..))

#include "Common.hs"

benchStr :: String
benchStr = "OsString"

w :: Int -> OsChar
w = case S.coercionToPlatformTypes of
  Left (co, _) -> coerceWith (sym co) . WindowsChar . fromIntegral
  Right (co, _) -> coerceWith (sym co) . PosixChar . fromIntegral

hashWord8 :: OsChar -> OsChar
hashWord8 = case S.coercionToPlatformTypes of
  Left (co, _) ->
    coerceWith (sym co) . WindowsChar . fromIntegral . hashInt . fromIntegral .
      getWindowsChar . coerceWith co
  Right (co, _) ->
    coerceWith (sym co) . PosixChar . fromIntegral . hashInt . fromIntegral .
      getPosixChar . coerceWith co

iw :: OsChar -> Int
iw = case S.coercionToPlatformTypes of
  Left (co, _) -> fromIntegral . getWindowsChar . coerceWith co
  Right (co, _) -> fromIntegral . getPosixChar . coerceWith co
