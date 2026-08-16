-- |
-- Module             : Trace.Hpc.ShowTix
-- Description        : The subcommand @hpc show@
-- License            : BSD-3-Clause
module Trace.Hpc.ShowTix (showtixPlugin) where

import Control.Monad (forM, forM_)
import qualified Data.Set as Set
import Trace.Hpc.Flags
  ( FlagOptSeq,
    Flags (includeMods),
    allowModule,
    excludeOpt,
    hpcDirOpt,
    includeOpt,
    outputOpt,
    readMixWithFlags,
    resetHpcDirsOpt,
    srcDirOpt,
    verbosityOpt,
  )
import Trace.Hpc.Mix (Mix (..), MixEntry)
import Trace.Hpc.Plugin (Plugin (..), hpcError)
import Trace.Hpc.Tix
  ( Tix (Tix),
    TixModule (..),
    getTixFileName,
    readTix,
    tixModuleName,
  )

showtixOptions :: FlagOptSeq
showtixOptions =
  excludeOpt
    . includeOpt
    . srcDirOpt
    . hpcDirOpt
    . resetHpcDirsOpt
    . outputOpt
    . verbosityOpt

showtixPlugin :: Plugin
showtixPlugin =
  Plugin
    { name = "show",
      usage = "[OPTION] .. <TIX_FILE> [<MODULE> [<MODULE> ..]]",
      options = showtixOptions,
      summary = "Show .tix file in readable, verbose format",
      implementation = showtixMain
    }

showtixMain :: Flags -> [String] -> IO ()
showtixMain _ [] = hpcError showtixPlugin "no .tix file or executable name specified"
showtixMain flags (prog : modNames) = do
  let hpcflags1 = flags {includeMods = Set.fromList modNames `Set.union` includeMods flags}

  optTixs <- readTix (getTixFileName prog)
  case optTixs of
    Nothing ->
      hpcError showtixPlugin $ "could not read .tix file : " ++ prog
    Just (Tix tixs) -> do
      -- Filter out TixModule's we are not interested in.
      let tixs_filtered = filter (allowModule hpcflags1 . tixModuleName) tixs
      -- Read the corresponding Mix file for each TixModule
      tixs_mixs <- forM tixs_filtered $ \tix -> do
        mix <- readMixWithFlags hpcflags1 (Right tix)
        pure (tix, mix)

      forM_ tixs_mixs printTixModule

printTixModule ::
  -- | A TixModule and the corresponding Mix-file
  (TixModule, Mix) ->
  IO ()
printTixModule (TixModule modName _ _ tixs, Mix _ _ _ _ entries) = do
  let enumerated :: [(Int, Integer, MixEntry)]
      enumerated = zip3 [(0 :: Int) ..] tixs entries

  forM_ enumerated $ \(ix, count, (pos, lab)) -> do
    putStrLn
      ( rjust 5 (show ix)
          ++ " "
          ++ rjust 10 (show count)
          ++ " "
          ++ ljust 20 modName
          ++ " "
          ++ rjust 20 (show pos)
          ++ " "
          ++ show lab
      )

-- | Pad input with space on the left.
--
-- >>> rjust 10 "hi"
-- "        hi"
rjust :: Int -> String -> String
rjust n str = replicate (n - length str) ' ' ++ str

-- | Pad input with space on the right.
--
-- >>> ljust 10 "hi"
-- "hi        "
ljust :: Int -> String -> String
ljust n str = str ++ replicate (n - length str) ' '
