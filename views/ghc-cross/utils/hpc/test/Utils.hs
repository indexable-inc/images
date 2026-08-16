module Utils (runCommands) where

import qualified Data.ByteString.Lazy.UTF8 as BS
import Data.List (intercalate)
import System.Process
    ( readCreateProcess, shell, CreateProcess(cwd) )

runCommands :: FilePath -> [String] -> IO BS.ByteString
runCommands dir cmds = do
    let combinedCmds = intercalate " && " cmds
    out <- readCreateProcess ((shell combinedCmds ) { cwd = Just dir }) ""
    pure (BS.fromString out)
