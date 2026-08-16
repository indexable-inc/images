{-# LANGUAGE CPP #-}
{-# LANGUAGE BangPatterns        #-}
{-# LANGUAGE PackageImports      #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE MagicHash           #-}
{-# LANGUAGE OverloadedStrings   #-}
{-# LANGUAGE QuasiQuotes         #-}
{-# LANGUAGE TypeApplications    #-}

#define OSSTR pstr
#define OS_STRING PosixString
#define OS_CHAR PosixChar


module BenchPosixString (benchMark) where

import           System.OsString.Posix                 (PosixString, pstr)
import qualified System.OsString.Posix                 as S
import           System.OsString.Internal.Types        (PosixChar(..))

#include "Common.hs"

benchStr :: String
benchStr = "PosixString"

w :: Int -> PosixChar
w i = PosixChar (fromIntegral i)

hashWord8 :: PosixChar -> PosixChar
hashWord8 (PosixChar w) = PosixChar . fromIntegral . hashInt . fromIntegral $ w

iw :: PosixChar -> Int
iw (PosixChar w) = fromIntegral w

