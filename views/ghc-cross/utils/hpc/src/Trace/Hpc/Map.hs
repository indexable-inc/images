-- |
-- Module             : Trace.Hpc.Map
-- Description        : The subcommand @hpc map@
-- Copyright          : Andy Gill, 2006
-- License            : BSD-3-Clause
module Trace.Hpc.Map (mapPlugin) where

import Trace.Hpc.Flags
import Trace.Hpc.Plugin
import Trace.Hpc.Tix

mapOptions :: FlagOptSeq
mapOptions =
  excludeOpt
    . includeOpt
    . outputOpt
    . mapFunOpt
    . mapFunOptInfo
    . unionModuleOpt
    . verbosityOpt

mapPlugin :: Plugin
mapPlugin =
  Plugin
    { name = "map",
      usage = "[OPTION] .. <TIX_FILE> ",
      options = mapOptions,
      summary = "Map a function over a single .tix file",
      implementation = mapMain
    }

mapMain :: Flags -> [String] -> IO ()
mapMain flags [first_file] = do
  let f = thePostFun (postFun flags)

  Just tix <- readTix first_file

  let (Tix inside_tix) = filterTix flags tix
  let tix' = Tix [TixModule m p i (map f t) | TixModule m p i t <- inside_tix]

  case outputFile flags of
    "-" -> print tix'
    out -> writeTix out tix'
mapMain _ [] = hpcError mapPlugin "no .tix file specified"
mapMain _ _ = hpcError mapPlugin "to many .tix files specified"
