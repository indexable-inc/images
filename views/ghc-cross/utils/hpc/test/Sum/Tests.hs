module Sum.Tests (sumTests) where

import qualified System.FilePath as FP
import Test.Tasty (TestTree, testGroup)
import qualified Data.ByteString.Lazy.UTF8 as BS
import Test.Tasty.Golden (goldenVsString)
import Utils (runCommands)


inputBaseDir :: FilePath
inputBaseDir = FP.joinPath ["test", "Sum", "input"]

goldBaseDir :: FilePath
goldBaseDir = FP.joinPath ["test", "Sum", "gold"]

-- | Tests of the "hpc sum" subcommand
sumTests :: TestTree
sumTests = testGroup "sum" [helpTextTest]


helpTextTest :: TestTree
helpTextTest = goldenVsString "Help" (goldBaseDir FP.</> "Help.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands "." ["hpc help sum"]
