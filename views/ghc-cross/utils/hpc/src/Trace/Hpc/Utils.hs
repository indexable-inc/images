{-# LANGUAGE GADTSyntax #-}

-- |
-- Module             : Trace.Hpc.Utils
-- Description        : Utility functions for hpc-bin
-- License            : BSD-3-Clause
module Trace.Hpc.Utils where

import Data.Char (isControl)
import qualified Data.Map as Map
import qualified Data.Set as Set
import Numeric (showHex)
import System.FilePath
import Trace.Hpc.Flags
import Trace.Hpc.Tix
import Trace.Hpc.Util

------------------------------------------------------------------------------

-- Spec: dropWhileEndLE p = reverse . dropWhile p . reverse
-- turns \n into ' '
dropWhileEndLE :: (a -> Bool) -> [a] -> [a]
dropWhileEndLE p = foldr (\x r -> if null r && p x then [] else x : r) []

-- | grab's the text behind a HpcPos;
grabHpcPos :: Map.Map Int String -> HpcPos -> String
grabHpcPos hsMap srcspan =
  case lns of
    [] -> error "grabHpcPos: invalid source span"
    [ln] ->
      take ((c2 - c1) + 1) $ drop (c1 - 1) ln
    hd : tl ->
      let lns1 = drop (c1 - 1) hd : tl
          lns2 = init lns1 ++ [take (c2 + 1) (last lns1)]
       in foldl1 (\xs ys -> xs ++ "\n" ++ ys) lns2
  where
    (l1, c1, l2, c2) = fromHpcPos srcspan
    lns =
      map
        ( \n -> case Map.lookup n hsMap of
            Just ln -> ln
            Nothing -> error $ "bad line number : " ++ show n
        )
        [l1 .. l2]

readFileFromPath :: (String -> IO String) -> String -> [String] -> IO String
readFileFromPath _ filename@('/' : _) _ = readFileUtf8 filename
readFileFromPath err filename path0 = readTheFile path0
  where
    readTheFile [] =
      err $
        "could not find "
          ++ show filename
          ++ " in path "
          ++ show path0
    readTheFile (dir : dirs) =
      catchIO
        (readFileUtf8 (dir </> filename))
        (\_ -> readTheFile dirs)

mergeTix :: MergeFun -> (Integer -> Integer -> Integer) -> Tix -> Tix -> Tix
mergeTix modComb f (Tix t1) (Tix t2) =
  Tix
    [ case (Map.lookup m fm1, Map.lookup m fm2) of
        -- todo, revisit the semantics of this combination
        (Just (TixModule _ hash1 len1 tix1), Just (TixModule _ hash2 len2 tix2))
          | hash1 /= hash2
              || length tix1 /= length tix2
              || len1 /= length tix1
              || len2 /= length tix2 ->
              error $ "mismatched in module " ++ m
          | otherwise ->
              TixModule m hash1 len1 (zipWith f tix1 tix2)
        (Just m1, Nothing) ->
          m1
        (Nothing, Just m2) ->
          m2
        _ -> error "impossible"
      | m <- Set.toList (theMergeFun modComb m1s m2s)
    ]
  where
    m1s = Set.fromList $ map tixModuleName t1
    m2s = Set.fromList $ map tixModuleName t2

    fm1 = Map.fromList [(tixModuleName tix, tix) | tix <- t1]
    fm2 = Map.fromList [(tixModuleName tix, tix) | tix <- t2]

-- | Simple data type to represent JSON documents.
-- Copied from: https://hackage.haskell.org/package/ghc-9.4.4/docs/GHC-Utils-Json.html
data JsonDoc where
  JSNull :: JsonDoc
  JSBool :: Bool -> JsonDoc
  JSInt :: Int -> JsonDoc
  JSString ::
    String ->
    -- | The 'String' is unescaped
    JsonDoc
  JSArray :: [JsonDoc] -> JsonDoc
  JSObject :: [(String, JsonDoc)] -> JsonDoc

-- | Class for types which can be converted to JSON
class ToJson a where
  json :: a -> JsonDoc

-- | returns the JSON-data as a single line
jsonToString :: JsonDoc -> String
jsonToString d = case d of
  JSNull -> "null"
  JSBool b -> if b then "true" else "false"
  JSInt n -> show n
  JSString s -> doubleQuotes $ escapeJsonString s
  JSArray as -> arrayToString as
  JSObject o -> objectToString o
  where
    brackets :: String -> String
    brackets s = "[" ++ s ++ "]"

    braces :: String -> String
    braces s = "{" ++ s ++ "}"

    doubleQuotes :: String -> String
    doubleQuotes s = "\"" ++ s ++ "\""

    arrayToString :: [JsonDoc] -> String
    arrayToString lst = brackets $ go lst
      where
        go [] = ""
        go [x] = jsonToString x
        go (x : xs) = jsonToString x ++ "," ++ go xs

    objectToString :: [(String, JsonDoc)] -> String
    objectToString lst = braces $ go lst
      where
        go [] = []
        go [(s, x)] = doubleQuotes s ++ ":" ++ jsonToString x
        go ((s, x) : os) =
          doubleQuotes s
            ++ ":"
            ++ jsonToString x
            ++ ","
            ++ go os

    escapeJsonString :: String -> String
    escapeJsonString = concatMap escapeChar
      where
        escapeChar '\b' = "\\b"
        escapeChar '\f' = "\\f"
        escapeChar '\n' = "\\n"
        escapeChar '\r' = "\\r"
        escapeChar '\t' = "\\t"
        escapeChar '"' = "\\\\\\\""
        escapeChar '\'' = "\\\'"
        escapeChar '\\' = "\\\\\\\\"
        escapeChar c | isControl c || fromEnum c >= 0x7f = uni_esc c
        escapeChar c = [c]

        uni_esc c = "\\u" ++ pad 4 (showHex (fromEnum c) "")

        pad n cs
          | len < n = replicate (n - len) '0' ++ cs
          | otherwise = cs
          where
            len = length cs

-- | Writes JSON data to file
writeJSON ::
  -- | Filepath to file
  FilePath ->
  -- | JSON data
  JsonDoc ->
  IO ()
writeJSON fp js = writeFileUtf8 fp $ jsonToString js
