{-# LANGUAGE CPP #-}
{-# LANGUAGE BangPatterns        #-}
{-# LANGUAGE PackageImports      #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE MagicHash           #-}
{-# LANGUAGE OverloadedStrings   #-}
{-# LANGUAGE QuasiQuotes         #-}
{-# LANGUAGE TypeApplications    #-}

#define OSSTR pstr
#define OS_STRING WindowsString
#define OS_CHAR WindowsChar


module BenchWindowsString (benchMark) where

import           System.OsString.Windows               (WindowsString, WindowsChar, pstr)
import qualified System.OsString.Windows               as S
import           System.OsString.Internal.Types        (WindowsChar(..))

#include "Common.hs"

benchStr :: String
benchStr = "WindowsString"

w :: Int -> WindowsChar
w i = WindowsChar (fromIntegral i)

hashWord8 :: WindowsChar -> WindowsChar
hashWord8 (WindowsChar w) = WindowsChar . fromIntegral . hashInt . fromIntegral $ w

iw :: WindowsChar -> Int
iw (WindowsChar w) = fromIntegral w

