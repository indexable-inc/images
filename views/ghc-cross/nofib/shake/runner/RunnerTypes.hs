{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE DeriveDataTypeable #-}
{-# LANGUAGE GeneralizedNewtypeDeriving #-}
{-# LANGUAGE DeriveGeneric #-}
{-# LANGUAGE TypeFamilies #-}

module RunnerTypes where

-- Standard libraries
import Data.Char
import Data.Maybe
import System.Process
import qualified System.Directory as IO
import Data.String (IsString)
import Options.Applicative

-- Shake - build system
import Development.Shake.FilePath hiding (exe)
import Development.Shake

import qualified Measurements as Ms

import GHC.Generics
import Control.DeepSeq

import GHC.Read as R
import Text.Read.Lex as L
import Text.ParserCombinators.ReadPrec (pfail)

---------------------------------------------------------------------
-- ARGUMENT PARSING - mostly based on CmdArgs

-- | Testnames are either
-- * A single tests name component (eg "rfib")
-- * A subpath eg. "real/eff"
-- * A fully qualified path e.g. "spectral/simple"
newtype TestName = TestName { unTestName :: String }
    deriving (Show, Eq, Ord, IsString, Generic, NFData)

newtype OConfig = OConfig String
    deriving (Show, Eq, Ord, IsString)

-- newtype GhcVersion = GhcVersion () deriving (Show,Typeable,Eq,Hashable,Binary,NFData)
type instance RuleResult OConfig = FilePath

-- | The directory which the given test lives in
testDir :: TestName -> FilePath
testDir (TestName dir) = dir

testLabel :: TestName -> Ms.Label
testLabel (TestName dir) = Ms.mkLabel dir

data Nofib
    = Build
        {clean :: Bool
        ,tests :: [TestName]
        ,threads :: Int
        ,compiler :: String
        ,compiler_args :: [String]
        ,output :: String -- ^ Where to put the results
        ,cachegrind :: Bool
        ,cachegrind_args :: [String]
        ,perf :: Bool
        ,perf_args :: [String]
        ,speed :: Speed
        ,rts_args :: [String]
        ,times :: Int
        ,keepGoing :: Bool
        ,useHackageHead :: Bool -- ^ Use hackage.head via nofib.head project file
        ,verbosity :: Int -- ^ Default 1
        }
    deriving (Show)

data Speed = Fast | Norm | Slow
    deriving (Show)

instance Read Speed where
  readPrec =
    parens
    ( do L.Ident s <- lexP
         case s of
           "Slow" -> return Slow
           "slow" -> return Slow
           "Norm" -> return Norm
           "norm" -> return Norm
           "Fast" -> return Fast
           "fast" -> return Fast
           _       -> pfail
    )

nofibMode :: Parser Nofib
nofibMode =
  Build
    <$> switch (short 'c' <> long "clean" <> help "Clean before building")
    <*> many (argument (TestName <$> str) (metavar "TEST" <> help "Tests to run (omit for all)"))
    <*> option auto (short 'j' <> long "threads" <> metavar "N" <> value 1 <> help "Number of threads, defaults to 1")
    <*> option str (short 'w' <> long "compiler" <> metavar "HC" <> value "ghc" <> help "Compiler to use, defaults to `ghc`")
    <*> many (option str (long "compiler-arg" <> help "Extra arguments to pass to the Compiler when building tests"))
    <*> option str (short 'o' <> long "output" <> metavar "DIR" <> help "Where to put created files under ./_make, defaults to {compiler version}")
    <*> switch (long "cachegrind" <> help "Run the tests under cachegrind")
    <*> many (option str (long "cachegrind-arg" <> help "Extra arguments to pass to cachegrind"))
    <*> switch (long "perf" <> help "Run the tests under `perf stat`")
    <*> (many (option str (long "perf-arg" <> help "Extra arguments to pass to `perf stat`"))
         <|> pure ["-e instructions,cycles,cache-misses,task-clock", "-r5"])
    <*> option auto (long "speed" <> short 's' <> value Norm <> help "Test speed (Fast,Norm,Slow)")
    <*> many (option str (long "rts-arg" <> help "Extra arguments to pass to runtime system when running"))
    <*> option auto (long "times" <> short 't' <> value 1 <> help "Number of times to run each test")
    <*> switch (long "keep-going" <> help "Ignore output missmatches")
    <*> switch (long "head" <> help "Use nofib.head project file for building test dependencies.")
    <*> option auto (long "verbosity" <> short 'v' <> short 'V' <> value 1 <> help "Verbosity, default = 1")


-- | Create a clean set of arguments, with any defaults filled in
nofibArgs :: IO Nofib
nofibArgs = do
    args <- execParser $ info (helper <*> nofibMode) (progDesc "nofib Benchmark Suite")
    print args
    case args of
        build@Build{..} -> do
            -- Turns "ghc" into /usr/bin/../ghc
            compiler' <- fromMaybe (error "Couldn't find GHC at" ++ compiler) <$> IO.findExecutable compiler
            compilerVer <- compilerVersion compiler
            output' <- return $ "_make" </> (if null output then compilerVer else output)
            return build{ output = output', compiler = compiler' }

-- | Find the default compiler string, e.g. ghc-7.4.1
compilerVersion :: FilePath -> IO String
compilerVersion compiler = do
    (_,stdout,_) <- readProcessWithExitCode compiler ["--version"] ""
    let ver = takeWhile (\x -> isDigit x || x == '.') $ dropWhile (not . isDigit) stdout
    return $ if null ver then "unknown" else ver
