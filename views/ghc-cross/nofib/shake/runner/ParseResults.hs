module ParseResults
  ( parseCodeSize
  , parseRtsStats
  ) where

import qualified Data.Map.Strict as M

-- | Parse text section size out of Berkley-format @size@ output
parseCodeSize :: String -> Integer
parseCodeSize content
  | header : sizes : _ <- lines content
  , text_hdr : _ <- words header
  , "text" == text_hdr || "__TEXT" == text_hdr
  = read $ head $ words sizes

  | otherwise = error "unrecognized size output"

parseRtsStats :: String -> M.Map String Double
parseRtsStats = foldMap parseValue . readPairs . dropFirstLine
  where
    parseValue (name, value)
      | (x, ""):_ <- reads value = M.singleton name x
      | otherwise = M.empty

    dropFirstLine = unlines . drop 1 . lines

    readPairs :: String -> [(String, String)]
    readPairs s
      | (x, _):_ <- reads s = x
      | otherwise = error $ "Failed to parse RTS statistics: " ++ s
