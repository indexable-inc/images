module ShowTix.Tests (showTixTests) where

import qualified System.FilePath as FP
import Test.Tasty (TestTree, testGroup)
import qualified Data.ByteString.Lazy.UTF8 as BS
import Test.Tasty.Golden (goldenVsString)
import Utils (runCommands)


inputBaseDir :: FilePath
inputBaseDir = FP.joinPath ["test", "ShowTix", "input"]

goldBaseDir :: FilePath
goldBaseDir = FP.joinPath ["test", "ShowTix", "gold"]

-- | Tests of the "hpc show" subcommand
showTixTests :: TestTree
showTixTests = testGroup "show" [recipTest, helpTextTest]

recipTest :: TestTree 
recipTest = goldenVsString "Recip" (goldBaseDir FP.</> "Recip.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands (inputBaseDir FP.</> "Recip") ["hpc show Recip.tix"]

helpTextTest :: TestTree
helpTextTest = goldenVsString "Help" (goldBaseDir FP.</> "Help.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands "." ["hpc help show"]
