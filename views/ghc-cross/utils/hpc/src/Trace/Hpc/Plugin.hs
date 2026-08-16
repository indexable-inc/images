-- |
-- Module             : Trace.Hpc.Plugin
-- Description        : The plugin type used by subcommands
-- License            : BSD-3-Clause
module Trace.Hpc.Plugin where

import System.Console.GetOpt
import System.Exit
import Trace.Hpc.Flags

data Plugin = Plugin
  { name :: String,
    usage :: String,
    options :: FlagOptSeq,
    summary :: String,
    implementation :: Flags -> [String] -> IO ()
  }

commandUsage :: Plugin -> IO ()
commandUsage plugin =
  putStrLn $
    "Usage: hpc "
      ++ name plugin
      ++ " "
      ++ usage plugin
      ++ "\n"
      ++ summary plugin
      ++ "\n"
      ++ if null (options plugin [])
        then ""
        else usageInfo "\n\nOptions:\n" (options plugin [])

hpcError :: Plugin -> String -> IO a
hpcError plugin msg = do
  putStrLn $ "Error: " ++ msg
  commandUsage plugin
  exitFailure
