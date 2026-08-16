{-# LANGUAGE OverloadedStrings #-}

import Data.Foldable
import Options.Applicative
import qualified Data.ByteString.Lazy as BS

import qualified Measurements as Ms
import Measurements (Measurements, Label)

args :: Parser (FilePath, [FilePath], Bool)
args =
  (,,)
  <$> option str (long "output" <> short 'o' <> help "output file")
  <*> some (argument str (metavar "FILE" <> help "results.json files"))
  <*> switch (long "json" <> short 'j' <> help "produce JSON output")

main :: IO ()
main = do
    (output, files, json) <- execParser $ info (helper <*> args) mempty
    trees <- mapM Ms.readFile files
    let result = fold trees
    if json
       then Ms.writeFile output result
       else Ms.writeFileTsv output result

