module Map.Tests (mapTests) where

import qualified System.FilePath as FP
import Test.Tasty (TestTree, testGroup)
import qualified Data.ByteString.Lazy.UTF8 as BS
import Test.Tasty.Golden (goldenVsString)
import Utils (runCommands)


inputBaseDir :: FilePath
inputBaseDir = FP.joinPath ["test", "Map", "input"]

goldBaseDir :: FilePath
goldBaseDir = FP.joinPath ["test", "Map", "gold"]

-- | Tests of the "hpc map" subcommand
mapTests :: TestTree
mapTests = testGroup "map" [helpTextTest]


helpTextTest :: TestTree
helpTextTest = goldenVsString "Help" (goldBaseDir FP.</> "Help.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands "." ["hpc help map"]
