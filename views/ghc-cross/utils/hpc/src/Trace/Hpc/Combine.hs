-- |
-- Module             : Trace.Hpc.Combine
-- Description        : The subcommand @hpc combine@
-- Copyright          : Andy Gill, 2006
-- License            : BSD-3-Clause
module Trace.Hpc.Combine (combinePlugin) where

import Trace.Hpc.Flags
import Trace.Hpc.Plugin
import Trace.Hpc.Tix
import Trace.Hpc.Utils

combineOptions :: FlagOptSeq
combineOptions =
  excludeOpt
    . includeOpt
    . outputOpt
    . combineFunOpt
    . combineFunOptInfo
    . unionModuleOpt
    . verbosityOpt

combinePlugin :: Plugin
combinePlugin =
  Plugin
    { name = "combine",
      usage = "[OPTION] .. <TIX_FILE> <TIX_FILE>",
      options = combineOptions,
      summary = "Combine two .tix files in a single .tix file",
      implementation = combineMain
    }

combineMain :: Flags -> [String] -> IO ()
combineMain flags [first_file, second_file] = do
  let f = theCombineFun (combineFun flags)

  Just tix1 <- readTix first_file
  Just tix2 <- readTix second_file

  let tix = mergeTix (mergeModule flags) f (filterTix flags tix1) (filterTix flags tix2)

  case outputFile flags of
    "-" -> print tix
    out -> writeTix out tix
combineMain _ _ = hpcError combinePlugin "need exactly two .tix files to combine"
