{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE TupleSections #-}

-- |
-- Module             : Trace.Hpc.Main
-- Description        : The main driver for Hpc
-- Copyright          : Andy Gill, 2007
-- License            : BSD-3-Clause
module Trace.Hpc.Main (main) where

import Control.Monad
import Data.Bifunctor
import Data.List (intercalate, partition, uncons)
import Data.List.NonEmpty (NonEmpty (..))
import Data.Maybe
import Data.Version
import Paths_hpc_bin
import System.Console.GetOpt
import System.Directory
import System.Environment
import System.Exit
import Trace.Hpc.Combine
import Trace.Hpc.Draft
import Trace.Hpc.Flags
import Trace.Hpc.Map
import Trace.Hpc.Markup
import Trace.Hpc.Overlay
import Trace.Hpc.Plugin
import Trace.Hpc.Report
import Trace.Hpc.ShowTix
import Trace.Hpc.Sum

------------------------------------------------------------------------------

helpList :: IO ()
helpList = do
  putStrLn $
    "Usage: hpc COMMAND ...\n\n"
      <> section "Commands" [helpPlugin]
      <> section "Reporting Coverage" [reportPlugin, markupPlugin]
      <> section "Processing Coverage files" [sumPlugin, combinePlugin, mapPlugin]
      <> section "Coverage Overlays" [overlayPlugin, draftPlugin]
      <> section "Others" [showtixPlugin, versionPlugin]
      <> ""
  putStrLn ""
  putStrLn "or: hpc @response_file_1 @response_file_2 ..."
  putStrLn ""
  putStrLn "The contents of a Response File must have this format:"
  putStrLn "COMMAND ..."
  putStrLn ""
  putStrLn "example:"
  putStrLn "report my_library.tix --include=ModuleA \\"
  putStrLn "--include=ModuleB"

-- | Print the summaries of all plugins belonging to a given section.
section ::
  -- | Name of the section.
  String ->
  -- | Plugins belonging to that section.
  [Plugin] ->
  String
section msg plugins = msg <> ":\n" <> unlines summaries
  where
    summaries = [take 14 ("  " <> name plugin <> repeat ' ') <> summary plugin | plugin <- plugins]

main :: IO ()
main = do
  args <- getArgs
  dispatch args

-- | The main dispatch function. It either accepts a valid command followed by a list of its arguments,
-- or a list of response files of the form '@filename'.
dispatch :: [String] -> IO ()
dispatch [] = do
  helpList
  exitSuccess
dispatch (txt : args0) = do
  case lookup txt hooks' of
    Just plugin -> do
      dispatchOnPlugin plugin args0
    _ -> case getResponseFileName txt of
      Nothing -> do
        dispatchOnPlugin helpPlugin (txt : args0)
      Just firstResponseFileName -> do
        let (responseFileNames', nonResponseFileNames) = partitionFileNames args0
        -- if arguments are combination of Response Files and non-Response Files, exit with error
        unless (null nonResponseFileNames) $ do
          putStrLn $
            "First argument '"
              <> txt
              <> "' is a Response File, "
              <> "followed by non-Response File(s): '"
              <> intercalate "', '" nonResponseFileNames
              <> "'"
          putStrLn $
            "When first argument is a Response File, "
              <> "all arguments should be Response Files."
          exitFailure
        dispatchOnResponseFiles (firstResponseFileName :| responseFileNames')
  where
    getResponseFileName :: String -> Maybe FilePath
    getResponseFileName s = do
      (firstChar, filename) <- uncons s
      if firstChar == '@'
        then pure filename
        else Nothing

    -- first member of tuple is list of Response File names,
    -- second member of tuple is list of all other arguments
    partitionFileNames :: [String] -> ([FilePath], [String])
    partitionFileNames xs =
      let hasFileName :: [(String, Maybe FilePath)]
          hasFileName = fmap (\x -> (x, getResponseFileName x)) xs
          (fileNames, nonFileNames) :: ([Maybe FilePath], [String]) =
            bimap (fmap snd) (fmap fst) $ partition (isJust . snd) hasFileName
       in (catMaybes fileNames, nonFileNames)

-- | Dispatch on a given list of response files.
dispatchOnResponseFiles :: NonEmpty FilePath -> IO ()
dispatchOnResponseFiles fps = do
  forM_ fps $ \responseFileName -> do
    exists <- doesPathExist responseFileName
    unless exists $ do
      putStrLn $ "Response File '" <> responseFileName <> "' does not exist"
      exitFailure

  -- read all Response Files
  responseFileNamesAndText :: NonEmpty (FilePath, String) <-
    forM fps $ \responseFileName ->
      fmap (responseFileName,) (readFile responseFileName)
  forM_ responseFileNamesAndText $ \(responseFileName, responseFileText) ->
    -- parse first word of Response File, which should be a command
    case uncons $ words responseFileText of
      Nothing -> do
        putStrLn $ "Response File '" <> responseFileName <> "' has no command"
        exitFailure
      Just (responseFileCommand, args1) -> case lookup responseFileCommand hooks' of
        -- check command for validity
        -- It is important than a Response File cannot specify another Response File;
        -- this is prevented
        Nothing -> do
          putStrLn $
            "Response File '"
              <> responseFileName
              <> "' command '"
              <> responseFileCommand
              <> "' invalid"
          exitFailure
        Just plugin -> do
          putStrLn $ "Response File '" <> responseFileName <> "':"
          dispatchOnPlugin plugin args1

-- | Dispatch on a given plugin and its arguments.
dispatchOnPlugin :: Plugin -> [String] -> IO ()
dispatchOnPlugin plugin args =
  case getOpt Permute (options plugin []) args of
    (_, _, errs) | not (null errs) -> do
      putStrLn "hpc failed:"
      sequence_ [putStr ("  " <> err) | err <- errs]
      putStrLn "\n"
      commandUsage plugin
      exitFailure
    (o, ns, _) -> do
      let flags = defaultFinalFlags (foldr ($) defaultFlags o)
      implementation plugin flags ns

hooks :: [Plugin]
hooks =
  [ helpPlugin,
    reportPlugin,
    markupPlugin,
    sumPlugin,
    combinePlugin,
    mapPlugin,
    showtixPlugin,
    overlayPlugin,
    draftPlugin,
    versionPlugin
  ]

hooks' :: [(String, Plugin)]
hooks' = [(name hook, hook) | hook <- hooks]

helpPlugin :: Plugin
helpPlugin =
  Plugin
    { name = "help",
      usage = "[<HPC_COMMAND>]",
      summary = "Display help for hpc or a single command",
      options = id,
      implementation = helpMain
    }

helpMain :: Flags -> [String] -> IO ()
helpMain _ [] = do
  helpList
  exitSuccess
helpMain _ (sub_txt : _) = do
  case lookup sub_txt hooks' of
    Nothing -> do
      putStrLn $ "no such HPC command: " <> sub_txt
      exitFailure
    Just plugin' -> do
      commandUsage plugin'
      exitSuccess

versionPlugin :: Plugin
versionPlugin =
  Plugin
    { name = "version",
      usage = "",
      summary = "Display version for hpc",
      options = id,
      implementation = versionMain
    }

versionMain :: Flags -> [String] -> IO ()
versionMain _ _ = putStrLn ("hpc tools, version " ++ showVersion version)
