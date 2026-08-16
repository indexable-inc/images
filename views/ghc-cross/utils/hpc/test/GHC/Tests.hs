module GHC.Tests (ghcTests) where
import System.Process
import System.Environment (getEnv)
import System.Directory (doesFileExist)
import Data.List
import Control.Monad (void)
import qualified Data.ByteString.Lazy.UTF8 as BS
import qualified System.FilePath as FP
import Test.Tasty
import Test.Tasty.HUnit
import Test.Tasty.Golden as G

-- | Tests coming from the GHC testsuite
ghcTests :: TestTree
ghcTests = testGroup "ghc-suite" $
    [ t10138
    , t2991
    , t17073
    , t20568
    , t11798
    ]

inputDir :: FilePath
inputDir = "test" FP.</> "GHC" FP.</> "inputs"

goldDir :: FilePath
goldDir = FP.joinPath ["test", "GHC", "gold"]

goldFile :: FilePath -> FilePath
goldFile file = goldDir FP.</> file

rm :: [FilePath] -> IO ()
rm files = void $ readCreateProcess ((shell (intercalate " && " . map ("rm -r " <>) $ files)) { cwd = Just inputDir }) ""

runCommands :: [IO String] -> IO String
runCommands commands = do
    cmds <- traverse id commands
    readCreateProcess ((shell $ intercalate " && " cmds) { cwd = Just inputDir }) "" 

hpc :: String -> IO String
hpc args = (<> args) . (<> " ") <$> getEnv "HPC"

ghc :: String -> IO String
ghc args = (<> args) . (<> " ") <$> getEnv "GHC"

exec :: String -> String
exec = ("./" <>)

{- Tests -}

t10138 :: TestTree
t10138 = testCaseInfo "T10138" $ runCommands . pure $ hpc . intercalate " " $ 
    ["report", "T10138.keepme.tix", "--hpcdir=.keepme.hpc.T10138"]

t2991 :: TestTree
t2991 = testCaseInfo "T2991" $ do
    runCommands commands
    clean
    pure ""
  where
    -- The .mix file for the literate module should have non-zero entries
    -- The 'grep' should exit with exit code 0
    commands = [ghc "-fhpc T2991.hs", pure $ exec "T2991", pure "grep -q cover_me .hpc/T2991LiterateModule.mix"]
    clean = rm ["*.hi", "*.o", "T2991", "T2991.tix", ".hpc"]  

t17073 :: TestTree
t17073 = goldenVsString "T17073" (goldFile "T17073.stdout") $ do 
        out1 <- runCommands commands
        clean
        pure $ BS.fromString out1
  where
    commands = [ ("LANG=ASCII " <>) <$> ghc "-fhpc -v0 T17073.hs"
                , pure $ exec "T17073"
                , hpc "report T17073"
                , hpc "version"
                , ("LANG=ASCII " <>) <$> hpc "markup T17073"
                ]
    clean = rm ["T17073.hi", "T17073.o", "T17073", "*.html", "T17073.tix", ".hpc"]

t20568 :: TestTree
t20568 = goldenVsString "T20568" (goldFile "T20568.stdout") $ do
        out <- runCommands commands
        clean
        pure $ BS.fromString $ last . lines $ out
  where
    commands = [ghc "T20568", pure $ exec "T20568"]
    clean = rm ["T20568.hi", "T20568.o", "T20568"]    

-- test that adding -fhpc triggers recompilation
t11798 :: TestTree
t11798 = goldenVsString "T11798" (goldFile "T11798.stdout") $ do
        c1 <- ghc "T11798.hs"
        out1 <- readCreateProcess ((shell c1) { cwd = Just inputDir }) ""
        c2 <- ghc "-fhpc T11798.hs"
        out2 <- readCreateProcess ((shell c2) { cwd = Just inputDir }) ""
        fmap (assertBool ".hpc/T11798.mix does not exist") $ doesFileExist $ 
            ".hpc" FP.</> "T11798.mix"
        clean
        pure $ BS.fromString $ unlines [last . lines $ out1,  last . lines $ out2]
  where
    clean = rm ["T11798.hi", "T11798.o", ".hpc"]
