module Report.Tests (reportTests) where

import qualified System.FilePath as FP
import Test.Tasty (TestTree, testGroup)
import qualified Data.ByteString.Lazy.UTF8 as BS
import Test.Tasty.Golden (goldenVsString)
import Utils (runCommands)



inputBaseDir :: FilePath
inputBaseDir = FP.joinPath ["test", "Report", "input"]

goldBaseDir :: FilePath
goldBaseDir = FP.joinPath ["test", "Report", "gold"]

-- | Tests for the `hpc report` subcommand
reportTests :: TestTree
reportTests = testGroup "report" [recipTestNormal, recipTestXML, helpTextTest]

recipTestNormal :: TestTree
recipTestNormal = goldenVsString "RecipNormal" (goldBaseDir FP.</> "RecipNormal.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands (inputBaseDir FP.</> "Recip") ["hpc report Recip.tix"]

recipTestXML :: TestTree
recipTestXML = goldenVsString "RecipXML" (goldBaseDir FP.</> "RecipXML.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands (inputBaseDir FP.</> "Recip") ["hpc report --xml-output Recip.tix"]

helpTextTest :: TestTree
helpTextTest = goldenVsString "Help" (goldBaseDir FP.</> "Help.stdout") go
  where
    go :: IO BS.ByteString
    go = runCommands "." ["hpc help report"]
