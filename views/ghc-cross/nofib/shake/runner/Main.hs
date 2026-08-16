{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE DeriveDataTypeable #-}
{-# LANGUAGE GeneralizedNewtypeDeriving #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE BangPatterns #-}

module Main(main) where

-- Standard libraries
import Control.Monad
import Data.Bifunctor
import Data.Char
import Data.Foldable
import Data.List
import Data.Maybe
import Control.Monad.IO.Class
import qualified Data.Map.Strict as M
import qualified System.Directory as IO
import qualified System.FilePath as FP
import qualified Data.ByteString.Char8 as BS
import qualified Data.ByteString.Lazy.Char8 as BSL
import System.Info hiding (compilerVersion)
import System.Exit
import qualified System.IO as IO

-- Shake - build system
import Development.Shake
import Development.Shake.FilePath hiding (exe)

import RunnerTypes
import qualified Measurements as Ms
import Measurements (Measurements, Label(..))
import qualified ParseResults
import qualified CachegrindParse
import qualified PerfStatParse

import Utils

-- | A handy shortcut.
ml :: String -> Label
ml = Ms.mkLabel

---------------------------------------------------------------------
-- TEST CONFIGURATION - which tests are available to run

-- | These are directories that we look into for tests by default.
testRoots :: [String]
testRoots = words "imaginary spectral real shootout"
            -- Note that we don't run the gc smp and parallel tests by default.
            -- See #24 for some of the reasoning. The short version is that they are not very
            -- stable run to run and are more sensitive to system load. So more care has to be
            -- taken to properly run these.
            -- Currently we value benchmark stability higher than including these in the default set
            -- therefore we skip these by default.

defaultNoFibHcOpts :: [String]
defaultNoFibHcOpts = words "-O2 -Wno-tabs"

{-  Note [Target Dependencies]
    ~~~~~~~~~~~~~~~~~~~~~~~~~~

The main build targets are the "*.results.tsv" files
which will contain compile and runtime metrics.

The results.tsv files depend on the benchmark configuration "config.txt"
and the actual executable.

The config files only depend on the (static) makefiles. They main work to create
them is done by convertConfig.

The executable depends on the ".depends" file which also gives us a list of
require object files, and their dependencies which it also depends on.

For there it's fairly straight forward.

-}

-- | Directories containing tests that the system can run.
getTestDirs :: [TestName] -> IO [TestName]
getTestDirs user_roots = do
    benchDirs <- if null user_roots
                    then concat <$> mapM getSubDirs testRoots
                    else concat <$> mapM getSubDirs (map unTestName user_roots)

    return $ map TestName benchDirs

-- Looking at a folder, determine paths to actual benchmarks.
--
-- We do so by parsing the makefile. If there is no Makefile there are no benchmarks.
-- If there is a SUBDIRS entry in the Makefile it gives the subfolders containing benchmarks
-- directly or indirectly via another Makefile with a SUBDIRS entry.
-- If there is a Makefile but no SUBDIRS entry then the path itself must be
-- a benchmark.
-- This only applies to benchmark folders. (real, shootout/binary-trees, ..)
getSubDirs :: FilePath -> IO [FilePath]
getSubDirs root = do
    hasMakefile <- IO.doesFileExist $ root </> "Makefile"
    if not hasMakefile then return [] else do
        config <- readConfig $ root </> "Makefile"
        let subdir_paths = words $ config "SUBDIRS"
        if null subdir_paths
            then return [root]
            else concat <$> mapM (\s -> getSubDirs (root </> s)) subdir_paths

---------------------------------------------------------------------
-- MAIN DRIVER

-- | Main program, just interpret the arguments and dispatch the tasks.
main :: IO ()
main = do

    -- Sanitize arguments
    args <- nofibArgs
    case args of
        Build{..} -> do
            when clean $
                removeDirectoryRecursive output

            tests' <- getTestDirs tests
            -- printM $ tests'
            putStrLn $ "Running: " ++ unwords (map unTestName tests')

            let shakeOpts = shakeOptions
                  { shakeThreads   = threads
                  , shakeFiles     = output ++ "/"
                  , shakeReport    = [output ++ "/shake_report.html"]
                  , shakeStaunch   = True
                  , shakeVerbosity = Development.Shake.Loud
                  }

            -- print shakeOpts
            shake shakeOpts $ buildRules (args {tests = tests'})
            putStrLn "Build completed"


-- | Rules to build the given tests. For each test, these are the files
--   we care about:
--
-- * config.txt - a cleaned up version of the configuration out of Makefile,
--   created by convertConfig.
--
-- * Main.exe - the actual binary, produced by ghc linking everything.
--
-- * .depend - lists all .o files required and their dependencies. Created by ghc -M.
--
-- * .hi/.o - files produced by ghc -c.
--
-- * .result - the stdout and stderr output from the build
--
--   Most complication comes from modules not named Main, which still produce
--   Main.o object files (I think ghc -M gets these wrong).
buildRules :: Nofib -> Rules ()
buildRules nofib@Build{..} = do
    pkgDbResource <- newResource "package db" 1

    -- _make/foo/bar -> bar
    let asSrcPath :: String -> String
        asSrcPath = drop (length output + 1)
    let resultDir :: TestName -> FilePath
        resultDir (TestName t) = output </> t

        -- Will turn _make/test/gc/linear/Matlib.o into gc/linear by
        -- Dropping the output directory prefix (asSrcPath)
        -- Finding a shared prefix between the file, and all tests.
        unoutput :: String -> TestName
        unoutput file =
                -- Drop output prefix
            let split_tests = map (splitPath . unTestName) tests :: [[FilePath]]
                split_file = splitPath (asSrcPath file)
                -- isPrefix is just isPrefixOf with equalFilePath instead of ==
                isPrefix [] _ = True
                isPrefix (p:fps) (f:fs)
                    | equalFilePath p f = isPrefix fps fs
                    | otherwise = False
                isPrefix _ _ = False
                m_test = listToMaybe $ filter (\x -> isPrefix x split_file) split_tests
                test = maybe (error $ "Failed to find test directory for file:" ++ file )
                            joinPath m_test
            in TestName test

            -- let f path
            --       | hasExtension path = f (takeDirectory path)
            --       | otherwise         = path
            -- in TestName . f . takeDirectory . drop (length output + 1)
    want $ concat
        [ [resultDir t </> "Main" <.> exe, resultDir t </> "config.txt"]
        | t <- tests
        ]
    when cachegrind $ want [ output </> "cachegrind.results.tsv" ]
    when perf $ want [ output </> "perf.results.tsv" ]
    when (not cachegrind && not perf) $ want [ output </> "run.results.tsv" ]

    -- Convenience phony rules
    sequence_
        [ unTestName t ~> need [resultDir t </> "results.tsv"]
        | t <- tests
        ]

    -- Aggregate rules
    let aggregateTarget :: [FilePath] -> FilePath -> Action ()
        aggregateTarget results out = do
          let results' = [ resultDir t </> fname
                         | t <- tests
                         , fname <- results
                         ]
        --   liftIO $ print out
        --   liftIO $ print results'
          need results'
          xs <- mapM readFileLines results'
          writeFileLines out (concat xs)

    (output </> "perf.results.tsv") %> aggregateTarget ["Main.perf.results.tsv"]
    (output </> "cachegrind.results.tsv") %> aggregateTarget ["Main.cachegrind.results.tsv"]
    (output </> "run.results.tsv") %> aggregateTarget ["Main" <.> "link.tsv", "Main.run.results.tsv"]

    let ghcPkg =  let (path, _ghcExec) = (FP.splitFileName compiler)
                  in path </> "ghc-pkg" <> IO.exeExtension

    -- Build dependency installation
    let buildDepsRoot = output </> "dep-packages" :: FilePath
        buildDepsStamp = buildDepsRoot </> ".stamp" :: FilePath
    let buildDepsPkgDb = do
          need [buildDepsStamp]
          compilerVer <- liftIO $ compilerVersion compiler
          -- Ideally we would rather point GHC at the package environment file
          -- created by cabal-install, but unfortunately I haven't been able to
          -- convince cabal v2-install to create such a file.
          -- The cabal env interface described here looks handy:
          -- https://github.com/haskell/cabal/issues/6481#issuecomment-620865817
          let pkgdb_path = buildDepsRoot </> ("ghc-" <> compilerVer) </> "package.db"
          return $ pkgdb_path
        buildDepsArgs _test = do
          pkgdb <- buildDepsPkgDb
          withResource pkgDbResource 1 $ do
            db_exists <- liftIO $ IO.doesPathExist pkgdb
            unless (db_exists) $  do
                cmd_ ghcPkg "init" pkgdb
          return [ "-package-db", pkgdb, "-no-user-package-db" ]

    -- Build all package dependencies. Write .stamp file to indicate
    -- this has been done afterwards. There is currently only one
    -- .stamp file per build directory.
    buildDepsStamp %> \out -> do
        configs <- mapM (getTestConfig nofib) tests
        let deps = nub $ foldMap (\config -> words $ config "SRC_DEPS") configs
        -- TODO: Invoking cabal in the way we do without any package argument fails.
        root <- liftIO $ IO.makeAbsolute buildDepsRoot
        unless (null deps) $ withResource pkgDbResource 1 $
            let project_file = if useHackageHead then "--project-file=cabal.project.head-hackage" else ""
            in
            cmd_ "cabal" project_file ("--store-dir=" <> root) "v2-install" "--lib" "-w" compiler "--allow-newer" deps ("-j"<> show threads)
        liftIO $ writeFile out ""

    -- Benchmark rules
    "_make//config.txt" %> \out -> do
        let test = unoutput out
        let src_dir = testDir test
        runVerbose nofib $ do
            printM $ "config:out:" ++ out
            printM $ "config:test:" ++ show test
            printM $ "config:src_dir:" ++ src_dir
        src <- lines . fuseLines <$> readFile' (src_dir </> "Makefile")
        writeFileLines out $ convertConfig src

    -- Main.exe : Link object files
    ["//Main" <.> exe, "//Main.link.tsv"] &%> \[out, resultsTsv] -> do

        let test = unoutput out :: TestName
            src_dir = testDir test          -- eg. spectral/simple
            obj_dir = output </> src_dir    -- eg. _make/foo/spectral/simple
        config <- getTestConfig nofib test
        let config_srcs = words $ config "SRCS"

        objs <-
            if null config_srcs
              then do
                -- This logic should probably go into config.txt generation
                deps <- readFile' $ replaceFileName out ".depends"
                let hs_objects = [ x | x <- words deps, takeExtension x == ".o"]
                -- Not recursive, the make system ignores files in subfolders.
                c_srcs <- getDirectoryFiles "" [src_dir ++ "/*.c"]
                need c_srcs
                runVerbose nofib $ printM $ "c_srcs" ++ show c_srcs
                let c_objects = map (\c_file -> replaceExtension (output </> c_file) "o") c_srcs
                return $ nub $ hs_objects ++ c_objects
              else do
                map (obj_dir </>) <$> mapM oFromSrc config_srcs

        liftIO $ runVerbose nofib $ do
            print "OBJS"
            mapM_ putStrLn objs

        need $ nub objs

        -- Metrics from object file compilations
        objectResults <- forM objs $ \o -> do
            liftIO $ Ms.readFile (o <.> "compile.tsv")

        -- Link executable
        compileArgs <- getTestCompileArgs nofib test
        deps_args <- buildDepsArgs test

        -- We pass the compiler_args as well, as we don't distinguish between link and compile time arguments
        cmd_ compiler $ ["-Rghc-timing","-rtsopts","-o", out] ++ objs ++ compileArgs ++ compiler_args ++ deps_args

        -- Report executable size
        Stdout out_err <- cmd "size" [out]
        let execSize = ParseResults.parseCodeSize $ BS.unpack out_err

        liftIO $ Ms.writeFileTsv resultsTsv
            $ fold objectResults
           <> Ms.singleton (testLabel test <> ml "executable size") (realToFrac execSize)

    ["_make//*.hi"] &%> \[hi] ->
        -- .hi files need the .o files, which in turn will also produce the .hi files
        -- We could avoid this by having separate rules for .o files built from C
        -- and haskell files (as shake docs recommend). But I couldn't bring myself
        -- to rewrite the logic required to do so yet again.
        -- TODO:
        -- If someone were to delete the .hi file, but not the .o file this means we
        -- *wont* rebuild it. So we should still refactor this at some point.
        need [(replaceExtension hi "o")]
    -- Compile object code
    ["_make//*.o","_make//*.o.compile.tsv"] &%> \[o, resultsTsv] -> do


        let test = unoutput o               -- eg. TestName "spectral/simple"
            src_dir = testDir test          -- eg. spectral/simple
            obj_dir = output </> src_dir    -- eg. _make/foo/spectral/simple
            o_name = drop (length obj_dir + 1) o -- eg. Main.o

        runVerbose nofib $ do
            printM "readConfig"

            printM $ "o:" ++ o
            printM $ "test:" ++ show test
            printM $ "obj_dir:" ++ obj_dir
            printM $ "src_dir:" ++ src_dir
            printM $ "o_name:" ++ o_name

        -- Deps are needed for build dependencies/src files
        -- But we only care about lines starting with the current .o file
        deps <- filter (\x -> head (words x) `equalFilePath` o) <$>
                    readFileLines (obj_dir </> ".depends")

        runVerbose nofib $ do
            printM $ "deps:"
            printM $ deps

        -- Figure out source location
        -- Haskell sources are recorded in .depends so look there first.
        -- It will be the first line mentioning the .o file.
        -- Otherwise we assume it's a C file.
        let src
                | (x:_) <- deps
                , (_ : ":" : rhs : _) <- words x
                = rhs
                | otherwise
                = replaceExtension (asSrcPath o) "c"

        -- We don't expect the source to change, but it makes debugging
        -- easier to fail here. If some started depending on a .cmm or cpp
        -- file in the future for example.
        need [src]

        runVerbose nofib $ do
            printM $ "src1:" ++ src

            printM $ "mod_name:" ++ o_name
            printM $ "src:" ++ src

        -- foo.o : foo.hs => foo.hs
        let needs = [ drop 2 . dropWhile (/= ':') $ x | x <- deps]
        runVerbose nofib $ liftIO $ do
            print "needs"
            mapM_ print needs
        need needs

        -- Compile it
        let ghc_rts_args = [ "+RTS", "--machine-readable", "-t"++o++".stats" ]
        compileArgs <- getTestCompileArgs nofib test
        deps_args <- buildDepsArgs test
        () <- cmd compiler $ ["-Rghc-timing","-c",src,"-w","-i"++obj_dir,"-odir="++obj_dir,"-hidir="++obj_dir,"-o",o]
                           ++ compileArgs ++ compiler_args ++ deps_args ++ ghc_rts_args

        -- Measure code size
        Stdout out_err <- cmd "size" [o]
        let objSize = realToFrac $ ParseResults.parseCodeSize $ BS.unpack out_err

        rtsStats <- liftIO $ readRtsStats $ o++".stats"
        liftIO $ Ms.writeFileTsv resultsTsv
            $ Ms.prefix (testLabel test <> ml "objects" <> Ms.mkLabel (takeFileName o))
            $ Ms.singleton (ml "size") objSize
           <> Ms.prefix (ml "rts stats") rtsStats

    -- Compute build dependencies
    "//.depends" %> \out -> do
        -- TODO: Maybe it would be better to look for C dependencies here, than in the main.exe rule
        -- but for now it will do.
        let test = unoutput out
            src_dir = testDir test          -- eg. spectral/simple
            obj_dir = output </> src_dir    -- eg. _make/foo/spectral/simple
        config <- readConfig' $ takeDirectory out </> "config.txt"
        compileArgs <- getTestCompileArgs nofib test
        deps_args <- buildDepsArgs test
        let config_srcs = words $ config "SRCS"
        let excluded_srcs = words $ config "EXCLUDED_SRCS"

        hs_files <- map (src_dir </>) <$> getDirectoryFiles src_dir ["/*.hs", "/*.lhs"]
        -- Note we do not exclude files in subfolders, as we don't scan these to begin with
        let is_excluded fp = any (`equalFilePath` (takeFileName fp)) excluded_srcs
        let src_files = if null config_srcs
                then filter (not . is_excluded) hs_files
                -- GHC's -M doesn't deal well with files other than .hs/.lhs files
                else map (src_dir </>) $ filter (\src -> takeExtension src `elem` [".hs", ".lhs"] ) config_srcs

        cmd_ compiler $
            [ "-w"
            , "-i" ++ src_dir
            , "-dep-makefile=" ++ out
            , "-dep-suffix", ""
            , "-odir="++obj_dir,"-hidir="++obj_dir -- Required for Main module .o handling.
                                                   -- See ghc issue #18575
            ]   ++ ["-M"] ++ src_files--src_dir </> config "MAIN"
                ++ compileArgs ++
                -- It's unlikely but possible that flags could affect dependencies.
                compiler_args ++
                deps_args
        src <- liftIO $ readFile out
        need [x | x <- words src, takeExtension x `elem` [".hs",".lhs",".h"]]

    -- Run tests normally
    ["//Main.run.results.tsv"] &%> \[resultsTsv] -> do
        runTest nofib ModeRun resultsTsv

    -- Run tests under perf stat
    ["//Main.perf.results.tsv"] &%> \[resultsTsv] -> do
        out' <- liftIO $ IO.canonicalizePath resultsTsv
        let resultN n = FP.replaceFileName out' ("Main.perf.result" <.> show n)
        let test = testFromResultTsv nofib resultsTsv
        let args n = ["perf", "stat"] <> perf_args <> ["-x,", ("--output="<>resultN n), "--"]
        let parse_perf n = do
                stats <- PerfStatParse.readPerfStat (resultN n)
                return $ Ms.fromList
                    [ (testLabel test <> ml "run" <> ml "perf" <> lbl, v)
                    | (eventName, vs) <- M.toList stats
                    , v <- vs
                    , let lbl = Ms.mkLabel $ PerfStatParse.getEventName eventName
                    ]

        runTest nofib (ModeWrapped args parse_perf) resultsTsv

    -- Run tests under cachegrind
    ["//Main.cachegrind.results.tsv"] &%> \[resultsTsv] -> do
        let test = testFromResultTsv nofib resultsTsv
            src_dir = testDir test          -- eg. spectral/simple
            obj_dir = output </> src_dir    -- eg. _make/foo/spectral/simple
        out' <- liftIO $ IO.canonicalizePath resultsTsv
        obj_dir' <- liftIO $ IO.canonicalizePath obj_dir
        let cachegrindOut n = FP.replaceFileName out' ("Main.cachegrind.result" <.> show n)
        let wrapper_args n =
              ["valgrind", "--tool=cachegrind"] <> cachegrind_args <>
              [ "--cachegrind-out-file="<>cachegrindOut n
              , "--log-file=" <> obj_dir' </> "cachegrind.log"
              ]

        let parse_cachegrind n = do
                stats <- CachegrindParse.parse (cachegrindOut n)
                return $ Ms.fromList
                    [ (testLabel test <> ml "run" <> ml "cachegrind" <> lbl, realToFrac v)
                    | (eventName, v) <- M.toList stats
                    , let lbl = Ms.mkLabel $ CachegrindParse.getEventName eventName
                    ]
        runTest nofib (ModeWrapped wrapper_args parse_cachegrind) resultsTsv

data RunMode  = ModeRun -- ^ Regular runtime measurement
              -- | Wrap the executable by a call to another executable.
              --
              -- For example perf or valgrind.
              -- The int is the n'th run
              | ModeWrapped (Int -> [String])
                            (Int -> IO (Measurements Double))


getWrapperArgs :: RunMode -> (Int -> [String])
getWrapperArgs ModeRun = const []
getWrapperArgs (ModeWrapped args _) = args

getWrapperParser :: RunMode -> (Int -> IO (Measurements Double))
getWrapperParser ModeRun = \_ -> return mempty
getWrapperParser (ModeWrapped _ parser) = parser

---------------------------------------------------------------------
-- RULES

-- | "foo/results.tsv" => TestName foo
testFromResultTsv :: Nofib -> String -> TestName
testFromResultTsv Build{..}=
    let f path
            | hasExtension path = f (takeDirectory path)
            | otherwise         = path
    in TestName . f . takeDirectory . drop (length output + 1)

runTest :: Nofib
        -> RunMode
        -> String
        -> Action ()
runTest nofib@Build{..} runMode resultsTsv = do
        -- Build executable first
        need [takeDirectory resultsTsv </> "config.txt", replaceExtensions resultsTsv exe]
        let test = testFromResultTsv nofib resultsTsv :: TestName
            src_dir = testDir test
            obj_dir = output </> src_dir

        has_boot_script <- doesFileExist (src_dir </> "boot.sh")
        when has_boot_script $ do
            abs_src_dir <- liftIO $ IO.makeAbsolute src_dir
            abs_obj_dir <- liftIO $ IO.makeAbsolute obj_dir
            Exit c <- cmd (Cwd abs_src_dir) "bash" "boot.sh" abs_src_dir abs_obj_dir compiler (map toLower $ show speed)
            unless (c == ExitSuccess) $ do
                fail $ "Boot script failed for:" ++ src_dir


        -- Construct benchmark invocation
        (stdin, args, stdout, stderr) <- getTestCmdline nofib test
        executable <- liftIO $ IO.canonicalizePath $ output </> src_dir </> "Main" <.> exe

        -- Create stats.0, stats.1, etc.
        let doRun :: Int -> Action ()
            doRun n = do
                let rtsStatsOut = executable <.> "stats" <.> show n
                (Exit c, Stderr (err :: BSL.ByteString), Stdout out) <-
                    cmd (Cwd $ src_dir) (EchoStdout False) (StdinBS stdin)
                        (getWrapperArgs runMode $ n)
                        executable args "+RTS" rts_args "--machine-readable" ("-t"++rtsStatsOut)
                unless (c == ExitSuccess) $ do
                    liftIO $ IO.hPutStrLn IO.stderr $ "Unexpect exit code:" ++ show c
                    unless (BSL.null err) $ liftIO $ do
                        BSL.hPutStr IO.stderr $ BSL.pack "Stderr output:\n"
                        BSL.hPutStrLn IO.stderr $ err
                    fail $ "Benchmark failed:" ++ src_dir


                -- We simply drop '\r' from both expected and actual output when comparing
                -- them. This avoids windows line ending issues.
                -- Given that checking the output is merely a sanity check
                -- that seems reasonable.
                let isNewlineChar x = x == '\r' || x == '\n'

                case stdout of
                  -- There is an stdout file, compare actual and expected stdout.
                  Just stdout'
                    | (BSL.filter (not . isNewlineChar) out) /= (BSL.filter (not . isNewlineChar) stdout') -> do
                        liftIO $ do
                            let actual_path = output </> src_dir </> "stdout.actual"
                            let expected_path = output </> src_dir </> "stdout.expected"
                            BSL.writeFile actual_path out
                            BSL.writeFile expected_path $ fromJust stdout
                            putStrLn "Start of actual output:"
                            BSL.putStrLn $ BSL.take 800 out
                            putStrLn "Start of expected output:"
                            BSL.putStrLn $ BSL.take 800 stdout'
                            putStrLn $ "Full expected/actual output written to " ++ (output </> src_dir)
                        unless keepGoing $ fail $ "Benchmark failed:" ++ src_dir ++ " unexpected stdout:^"
                  -- No stdout file, ignore stdout
                  _ -> return ()

                case stderr of
                  -- Compare stderr file and actual stderr
                  Just stderr'
                    | (BSL.filter (not . isNewlineChar) err) /= (BSL.filter (not . isNewlineChar) stderr') -> do
                        liftIO $ do
                            let actual_path = output </> src_dir </> "stderr.actual"
                            let expected_path = output </> src_dir </> "stderr.expected"
                            BSL.writeFile actual_path err
                            BSL.writeFile expected_path $ fromJust stderr
                            putStrLn "Start of actual error output:"
                            BSL.putStrLn $ BSL.take 800 err
                            putStrLn "Start of expected error output:"
                            BSL.putStrLn $ BSL.take 800 stderr'
                            putStrLn $ "Full expected/actual error output written to " ++ (output </> src_dir)
                        fail $ "Benchmark failed:" ++ src_dir ++ " unexpected stderr:see above"
                  -- No stderr file, any error output is considered a failure
                  Nothing
                    | (not $ BSL.null err) -> do
                        fail $ "Benchmark failed:" ++ src_dir ++ " unexpected stderr:" ++ show err
                  _ -> return ()

        -- Run benchmarks n times
        forM_ [1..times] $ \n -> doRun n

        -- Read results
        let doParse :: Int -> Action (Measurements Double)
            doParse n = do
                let rtsStatsOut = executable <.> "stats" <.> show n
                wrapper_measurements <- liftIO (getWrapperParser runMode $ n) :: Action (Measurements Double)
                rtsStats <- liftIO $ readRtsStats rtsStatsOut
                return $
                    wrapper_measurements <>
                    (Ms.prefix (testLabel test <> ml "run" <> ml "rts stats") rtsStats)

        measurements <- foldMap doParse [1..times :: Int] :: Action (Measurements Double)
        liftIO $ Ms.writeFileTsv resultsTsv $ measurements

getTestConfig :: Nofib -> TestName -> Action (String -> String)
getTestConfig Build{..} test =
    readConfig' $ output </> testDir test </> "config.txt"

getTestCompileArgs :: Nofib -> TestName -> Action [String]
getTestCompileArgs nofib test = do
    config <- getTestConfig nofib test
    return $ defaultNoFibHcOpts
         ++ words (config "SRC_HC_OPTS")
         ++ [ "-package-env", "-" ]
         ++ concat [ ["-package", pkg] | pkg <- words (config "SRC_DEPS") ]

getModeArgs :: (String -> String) -> Speed -> [String]
getModeArgs benchSettings speed = words $
    case speed of
        Slow -> with_default "SLOW_OPTS"
        Norm -> benchSettings "NORM_OPTS"
        Fast -> with_default "FAST_OPTS"
    where
        with_default mode_key
            | settings <- (benchSettings mode_key)
            , not (null settings)
            = settings
            | otherwise
            = (benchSettings "NORM_OPTS")

-- | Get stdin, arguments, expected stdout for benchmarks
getTestCmdline :: Nofib -> TestName -> Action (BSL.ByteString, [String], Maybe (BSL.ByteString), Maybe (BSL.ByteString))
getTestCmdline nofib@Build{..} test = do
    config <- readConfig' $ output </> src_dir </> "config.txt"

    -- Mode/Speed args default to normal mode.
    let speed_args = getModeArgs config speed

    -- print config
    let args = words (config "PROG_ARGS")
            ++ speed_args

    -- Check if there is stdin data, default to ""
    stdin_path <-
      let s = config "STDIN_FILE"
      in if s == ""
            then liftIO $ grab "stdin"
            else pure $ Just $ testDir test </> s
    liftIO $ runVerbose nofib $ putStrLn $ "test " <> unTestName test <> " stdin: " <> show stdin_path
    stdin <- liftIO $ maybe (pure BSL.empty) BSL.readFile stdin_path

    -- Check if there is an stdout file. Default to Nothing
    stdout_path <-
      let s = config "STDOUT_FILE"
      in if null s
            then liftIO $ grab "stdout"
            else pure $ Just $ testDir test </> s :: Action (Maybe FilePath)
    liftIO $ runVerbose nofib $ putStrLn $ "test " <> unTestName test <> " stdout: " <> show stdout_path
    stdout <- liftIO $ sequence (BSL.readFile <$> stdout_path)

    -- Check if there is an stdout file. Default to Nothing
    stderr_path <-
      let s = config "STDERR_FILE"
      in if null s
            then liftIO $ grab "stderr"
            else pure $ Just $ testDir test </> s :: Action (Maybe FilePath)
    liftIO $ runVerbose nofib $ putStrLn $ "test " <> unTestName test <> " stderr: " <> show stderr_path
    stderr <- liftIO $ sequence (BSL.readFile <$> stderr_path)

    return (stdin, args, stdout, stderr)
  where
    src_dir = testDir test
    -- Grab stdin/out
    grab :: String -> IO (Maybe FilePath)
    grab ext = do
        let s = [-- Generated stdin/out files from output directory
                 output </> src_dir </> takeFileName (unTestName test) <.> map toLower (show speed) ++ ext
                ,output </> src_dir </> takeFileName (unTestName test) <.> ext

                -- Constant stdin/out files in src directory
                ,src_dir </> takeFileName (unTestName test) <.> map toLower (show speed) ++ ext
                ,src_dir </> takeFileName (unTestName test) <.> ext
                ]
        ss <- filterM IO.doesFileExist s
        return $ listToMaybe ss

readRtsStats :: FilePath -> IO (Measurements Double)
readRtsStats fname = do
    rtsStats <- ParseResults.parseRtsStats <$> readFile fname
    return $ Ms.fromList $ map (first Ms.mkLabel) $ M.toList rtsStats


---------------------------------------------------------------------
-- CONFIGURATION UTILITIES
-- The Makefile's are slurped for configuration, to produce a cleaned-up config file

-- | Given the source of a Makefile, slurp out the configuration strings.
convertConfig :: [String] -> [String]
convertConfig xs =
      [ k ++ " = " ++ v
      | (k, v) <- M.toList vars
      ]
    where
        vars =
            M.fromListWith (\a b -> a ++ " " ++ b)
            [ (a, b)
            | x <- xs
            , let (a,b) = separate x
            , a `elem` keep
            ]
        keep = words $  "PROG_ARGS SRCS EXCLUDED_SRCS SRC_HC_OPTS SLOW_OPTS " ++
                        " NORM_OPTS FAST_OPTS STDIN_FILE SRC_DEPS SUBDIRS"

        separate x = (name,rest)
            where (name,x2) = span (\c -> isAlpha c || c == '_') x
                  rest = dropWhile isSpace $ dropWhile (`elem` "+=") $ dropWhile isSpace x2


-- | Read a configuration file (new format) into a function supplying options.
readConfig :: FilePath -> IO (String -> String)
readConfig x = do
    -- printM $ "readConfig:" ++ x
    src <- fuseLines <$> readFile x
    -- !_ <- return $ length src
    let res = [ (reverse $ dropWhile isSpace $ reverse a, dropWhile isSpace $ drop 1 b)
              | y <- lines src
              , let (a,b) = break (== '=') y
              ]
    return $ \k -> fromMaybe "" $ lookup k res

-- Eliminate line breaks preceded by \ which allows multiline statements in make files
fuseLines :: String -> String
fuseLines [] = []
fuseLines [x] = [x]
fuseLines ('\\':'\n':xs) =
    fuseLines xs
fuseLines ('\\':'\r':'\n':xs) =
    fuseLines xs
fuseLines ('\\':'\r':xs) =
    fuseLines xs
fuseLines (x:xs) =
    x : fuseLines xs

-- | readConfig lifted into the Action monad.
readConfig' :: FilePath -> Action (String -> String)
readConfig' x = do
    -- printM $ "readConfig':" ++ x
    need [x]
    liftIO $ readConfig x


---------------------------------------------------------------------
-- GENERAL UTILITIES

-- | The executable extension on this platform.
exe :: String
exe = if os == "mingw32" then "exe" else ""


-- | Like the standard removeDirectoryRecursive, but doesn't fail if the path is missing.
removeDirectoryRecursive :: FilePath -> IO ()
removeDirectoryRecursive x = do
    b <- IO.doesDirectoryExist x
    when b $ IO.removeDirectoryRecursive x

printM :: (MonadIO m, Show a) => a -> m ()
printM x = liftIO $ print x
