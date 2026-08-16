module Main (main) where

import           Test.Tasty.Bench
import qualified BenchOsString
import qualified BenchPosixString
import qualified BenchWindowsString


main :: IO ()
main = do
  defaultMain [ BenchOsString.benchMark
              , BenchPosixString.benchMark
              , BenchWindowsString.benchMark
              ]

