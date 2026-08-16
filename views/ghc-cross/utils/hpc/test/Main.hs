module Main where

import Test.Tasty ( defaultMain, testGroup, TestTree )
import GHC.Tests (ghcTests)
import ShowTix.Tests (showTixTests)
import Report.Tests (reportTests)
import Version.Tests (versionTests)
import Map.Tests (mapTests)
import Sum.Tests (sumTests)
import Combine.Tests (combineTests)

main :: IO ()
main = defaultMain tests

tests :: TestTree
tests = testGroup "hpc" [ ghcTests, showTixTests, reportTests, versionTests, mapTests, sumTests, combineTests]
