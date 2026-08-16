-- |
-- Module             : Trace.Hpc.Sum
-- Description        : The subcommand @hpc sum@
-- Copyright          : Andy Gill, 2006
-- License            : BSD-3-Clause
module Trace.Hpc.Sum (sumPlugin) where

import Control.DeepSeq
import Control.Monad
import Trace.Hpc.Flags
import Trace.Hpc.Plugin
import Trace.Hpc.Tix
import Trace.Hpc.Utils

sumOptions :: FlagOptSeq
sumOptions =
  excludeOpt
    . includeOpt
    . outputOpt
    . unionModuleOpt
    . verbosityOpt

sumPlugin :: Plugin
sumPlugin =
  Plugin
    { name = "sum",
      usage = "[OPTION] .. <TIX_FILE> [<TIX_FILE> [<TIX_FILE> ..]]",
      options = sumOptions,
      summary = "Sum multiple .tix files in a single .tix file",
      implementation = sumMain
    }

sumMain :: Flags -> [String] -> IO ()
sumMain _ [] = hpcError sumPlugin "no .tix file specified"
sumMain flags (first_file : more_files) = do
  Just tix <- readTix first_file

  tix' <- foldM (mergeTixFile flags (+)) (filterTix flags tix) more_files

  case outputFile flags of
    "-" -> print tix'
    out -> writeTix out tix'

mergeTixFile :: Flags -> (Integer -> Integer -> Integer) -> Tix -> String -> IO Tix
mergeTixFile flags fn tix file_name = do
  Just new_tix <- readTix file_name
  return $! force $ mergeTix (mergeModule flags) fn tix (filterTix flags new_tix)
