module Combine.Tests (combineTests) where

import qualified System.FilePath as FP
import Test.Tasty (TestTree, testGroup)
import qualified Data.ByteString.Lazy.UTF8 as BS
import Test.Tasty.Golden (goldenVsString)
import Utils (runCommands)


inputBaseDir :: FilePath
inputBaseDir = FP.joinPath ["test", "Combine", "input"]

goldBaseDir :: FilePath
goldBaseDir = FP.joinPath ["test", "Combine", "gold"]

-- | Tests of the "hpc combine" subcommand
combineTests :: TestTree
combineTests = testGroup "combine" [helpTextTest]


helpTextTest :: TestTree
helpTextTest = goldenVsString "Help" (goldBaseDir FP.</> "Help.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands "." ["hpc help combine"]
