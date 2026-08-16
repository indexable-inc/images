{-# LANGUAGE OverloadedStrings #-}

module PerfStatParse (readPerfStat, EventName(..)) where

import qualified Data.Text as T
import qualified Data.Text.IO as TIO
import qualified Data.Map as M

newtype EventName = EventName { getEventName :: String }
                  deriving (Show, Eq, Ord)

-- | Read metrics from @perf stat -x,@ invocation.
readPerfStat :: FilePath -> IO (M.Map EventName [Double])
readPerfStat path = do
    parsePerfStat <$> TIO.readFile path

parsePerfStat :: T.Text -> M.Map EventName [Double]
parsePerfStat = M.unionsWith (++) . map parseLine . T.lines
  where
    parseLine line
      | "#" `T.isPrefixOf` line = mempty
      | value:_:metric:_ <- T.splitOn "," line
      , (val, "") : _ <- reads $ T.unpack value
      = M.singleton (EventName $ T.unpack metric) [val]
      | otherwise = mempty
