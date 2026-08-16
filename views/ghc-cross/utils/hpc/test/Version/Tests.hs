module Version.Tests (versionTests) where

import qualified System.FilePath as FP
import Test.Tasty (TestTree, testGroup)
import qualified Data.ByteString.Lazy.UTF8 as BS
import Test.Tasty.Golden (goldenVsString)
import Utils (runCommands)


goldBaseDir :: FilePath
goldBaseDir = FP.joinPath ["test", "Version", "gold"]


-- | Tests for the `hpc version` subcommand
versionTests :: TestTree
versionTests = testGroup "version" [versionTest, helpTextTest]

versionTest :: TestTree
versionTest = goldenVsString "Version" (goldBaseDir FP.</> "Version.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands "." ["hpc version"]

helpTextTest :: TestTree
helpTextTest = goldenVsString "Help" (goldBaseDir FP.</> "Help.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands "." ["hpc help version"]

