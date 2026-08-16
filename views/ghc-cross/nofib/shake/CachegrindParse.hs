{-# LANGUAGE GeneralizedNewtypeDeriving #-}

module CachegrindParse where

import Data.Maybe
import qualified Data.Map as M

newtype EventName = EventName { getEventName :: String }
                  deriving (Show, Eq, Ord)

parse :: FilePath -> IO (M.Map EventName Integer)
parse fname = parse' <$> readFile fname

parse' :: String -> M.Map EventName Integer
parse' content =
    M.fromList $ zip summary events
  where
    events = case mapMaybe isEventList $ lines content of
               [] -> error "No event list found"
               [x] -> x
               _ -> error "More than one event list found"
    summary = case mapMaybe isSummaryList $ lines content of
               [] -> error "No event summary found"
               [x] -> x
               _ -> error "More than one event summary found"

    isEventList :: String -> Maybe [Integer]
    isEventList line
      | "summary:" : rest <- words line = Just $ map read rest
      | otherwise = Nothing

    isSummaryList :: String -> Maybe [EventName]
    isSummaryList line
      | "events:" : rest <- words line = Just $ map EventName rest
      | otherwise = Nothing

