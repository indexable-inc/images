module Utils where

import Control.Monad
import System.FilePath as IO
import Development.Shake
import Data.Maybe

import Prelude hiding (lines)

import RunnerTypes
-------------------------------------------------------------------------------
-- Finding source files for a given .o file
-------------------------------------------------------------------------------

runWithVerbosity :: Monad m => Nofib -> Int -> m () -> m ()
runWithVerbosity Build { verbosity = conf_verbosity} v log_action = do
    when (conf_verbosity > v) log_action

runVerbose :: Monad m => Nofib -> m () -> m ()
runVerbose n act = runWithVerbosity n 2 act


-- | dropPathPrefix "foo" "foo/bar" == "bar"
dropPathPrefix :: [Char] -> [Char] -> FilePath
dropPathPrefix prefix path =
    let path_segments        = splitPath path
        prefix_segments = splitPath prefix
        filter_segments (pre:pres) (seg:segs)
            | equalFilePath pre seg
            = filter_segments pres segs
        filter_segments _ segs = segs
    in  joinPath $ filter_segments prefix_segments path_segments

-- | Try to find the source in the .depends file
--
-- E.g. gc/linear/Matrix.o => Just gc/linear/Matrix.lhs
-- We already filtered out any lines *not* specifying a dep
-- for the current file. So if there are deps the first line will
-- give the source file.
getHaskellSrcFromDeps :: String -> [String] -> Maybe String
getHaskellSrcFromDeps _    (x:_)
    | (_ : ":" : rhs : _) <- words x
    = Just rhs
getHaskellSrcFromDeps _    _ = Nothing

-- -- There is always one Main.o file for which the source file is recorded
-- -- in the .config file
-- getMainSrc :: String -> String -> (String -> String) -> Maybe String
-- getMainSrc "Main.o" dir config = Just $! (dir </> (config "MAIN"))
-- getMainSrc _ _      _        = Nothing

-- | Generic case, for foo.o look for foo.c
--
-- Haskell source files should be recorded in the .depends file
-- so only C files should remain.
-- No maybe as a source file is required. We just
-- fail if we can't find it.
getCSrc :: String -> String -> String
getCSrc o_file dir = dir </> replaceExtension o_file ".c"

-------------------------------------------------------------------------------
-- Deriving the name of the .o file a given source file
-------------------------------------------------------------------------------

-- | Turn Foo.[hs|lhs|c] into Foo.o
oFromSrc :: FilePath -> Action FilePath
oFromSrc src = do
    let (file,ext) = splitExtension src
    unless (ext `elem` [".hs", ".lhs", ".c"]) $
        putWarn $ "Unknown source type " ++ ext
    return $ replaceExtension file "o"
