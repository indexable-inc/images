{-# LANGUAGE FlexibleInstances #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE ScopedTypeVariables #-}

import Debug.Trace

import Data.Align (Align, alignWith)
import Data.Maybe
import Data.These
import Data.Char
import Data.List (intercalate, group)
import Data.Foldable
import Numeric
import qualified Data.Sequence as Seq
import qualified Data.Tree as Tree
import Data.Tree (Tree)
import qualified Data.Map.Strict as M
import qualified Data.Text as T
import System.Directory
import System.FilePath
import Text.Tabular
import qualified Text.Tabular.AsciiArt as AsciiArt
import qualified Text.Tabular.Csv as Csv
import qualified Text.Tabular.Latex as Latex
import Options.Applicative as O
import System.Console.ANSI.Codes as ANSI

import qualified Measurements as Ms
import Measurements (Measurements, Label)
import LabelMatch

data Mode = RelativeMode | AbsoluteMode

args :: Parser (Mode, OutputDest, OutputFormat, OutputOptions, [FilePath])
args =
    (,,,,)
    <$> flag RelativeMode AbsoluteMode (long "absolute" <> short 'a' <> help "show absolute metrics")
    <*> outputDest
    <*> formatOption
    <*> outputOptions
    <*> some (argument str (metavar "FILE" <> help "results.json file"))
  where
    formatOption :: Parser OutputFormat
    formatOption = option (str >>= parse) (long "format" <> short 'f' <> help "Output format (one of markdown, ascii, csv, latex)" <> O.value FmtAsciiArt)
      where
        parse "markdown" = pure FmtMarkdown
        parse "ascii" = pure FmtAsciiArt
        parse "csv" = pure FmtCsv
        parse "latex" = pure FmtLatex
        parse _ = fail "Unknown --format"

    outputDest :: Parser OutputDest
    outputDest =
      option (ToSingleFile <$> str) (long "output" <> short 'o' <> help "write output to single file")
      <|> option (ToManyFiles <$> str) (long "output-dir" <> short 'O' <> help "write output to many files, one per table")
      <|> pure ToStdout

    outputOptions :: Parser OutputOptions
    outputOptions =
      OutputOptions
      <$> switch (long "fancy-chars" <> short 'f' <> help "Use proper typography")
      <*> switch (long "color" <> short 'c' <> help "Enable color output")

renderGitLabMarkdown :: (rh -> String) -> (ch -> String) -> (a -> String) -> Table rh ch a -> String
renderGitLabMarkdown renderRow renderCol renderCell (Table rows cols cells) =
    unlines $ [joinCols header, joinCols headerSep] ++ cells'
  where
    prepend x = (x :)
    joinCols xs = "| " ++ intercalate " | " xs ++ " |"
    header = prepend "" $ map renderCol (headerContents cols)
    headerSep = map (const " -------- ") header
    emphasize x = "*"++x++"*"
    cells' = zipWith (\rh row -> joinCols
                                 $ prepend (emphasize $ renderRow rh)
                                 $ map renderCell row
                     ) (headerContents rows) cells

data OutputFormat = FmtMarkdown | FmtAsciiArt | FmtCsv | FmtLatex

render :: OutputFormat
       -> (rh -> String) -> (ch -> String) -> (a -> String)
       -> Table rh ch a
       -> String
render FmtMarkdown = renderGitLabMarkdown
render FmtAsciiArt = AsciiArt.render
render FmtCsv = Csv.render
render FmtLatex = Latex.render

-- | Pick out the interesting part of a set of filenames for use as a friendly
-- name. Essentially we just break up all paths into their components and drop
-- all components common to all.
friendlyNameFiles :: [FilePath] -> M.Map MetricFile FilePath
friendlyNameFiles files =
    M.fromList
    [ (MetricFile $ joinPath $ toList name, path)
    | (path, name) <- M.toList friendlyNames
    ]
  where
    friendlyNames :: M.Map FilePath (Seq.Seq String)
    friendlyNames =
        fmap (fmap snd . Seq.filter (not . fst) . Seq.zip isCommon) splitPaths
      where
        isCommon :: Seq.Seq Bool
        isCommon = Seq.fromList $ fmap isCommonComp [0..maximum $ fmap length splitPaths]

    splitPaths :: M.Map FilePath (Seq.Seq String)
    splitPaths = M.fromList [ (f, Seq.fromList $ splitPath f) | f <- files ]

    isCommonComp :: Int -> Bool
    isCommonComp n = allEqual $ M.elems $ fmap (Seq.!? n) splitPaths

    allEqual :: Eq a => [a] -> Bool
    allEqual xs = length (group xs) == 1

mapColHeader :: (ch -> ch') -> Table rh ch a -> Table rh ch' a
mapColHeader f (Table rh ch xs) = Table rh (fmap f ch) xs

mapRowHeader :: (rh -> rh') -> Table rh ch a -> Table rh' ch a
mapRowHeader f (Table rh ch xs) = Table (fmap f rh) ch xs

mapTable :: (a -> b) -> Table rh ch a -> Table rh ch b
mapTable f (Table rh ch xs) = Table rh ch (map (map f) xs)

mapSemiTable :: (a -> b) -> SemiTable h a -> SemiTable h b
mapSemiTable f (SemiTable h xs) = SemiTable h (map f xs)

data OutputOptions = OutputOptions { fancyChars, color :: Bool }

simpleOutput :: OutputOptions
simpleOutput = OutputOptions False False

class ToRowLabel a where toRowLabel :: a -> String
instance ToRowLabel String where toRowLabel = id
instance ToRowLabel Label where toRowLabel = T.unpack . Ms.encodeLabel
instance (ToRowLabel a, ToRowLabel b) => ToRowLabel (a,b) where
  toRowLabel (a,b) = toRowLabel a ++ " // " ++ toRowLabel b

withColor :: OutputOptions -> ANSI.Color -> ShowS -> ShowS
withColor opts c s
  | color opts =
       showString start . s . showString end
  | otherwise = s
  where
    start = ANSI.setSGRCode [ANSI.SetColor ANSI.Foreground ANSI.Vivid c]
    end = ANSI.setSGRCode [ANSI.Reset]

showGFloat' :: RealFloat a => OutputOptions -> Maybe Int -> a -> ShowS
showGFloat' opts prec x = prettySign opts x . showGFloat prec (abs x)

showGFloatAbsolute :: RealFloat a => OutputOptions -> Maybe Int -> a -> ShowS
showGFloatAbsolute opts prec x
  | signum x < 0 = prettySign opts x . showGFloat prec (abs x)
  | otherwise    = showGFloat prec (abs x)


prettySign :: (Ord a, Num a) => OutputOptions -> a -> ShowS
prettySign opts x
  | x < 0 = if fancyChars opts then showChar '−' else showChar '-'
  | x > 0 = showChar '+'
  | otherwise = id

-- | The presentation name of a metric file.
newtype MetricFile = MetricFile { metricFileFriendlyName :: String }
                   deriving (Ord, Eq, Show)

-- | Where to write output?
data OutputDest = ToStdout | ToSingleFile FilePath | ToManyFiles FilePath

initializeOutputDest :: OutputDest -> IO ()
initializeOutputDest ToStdout = return ()
initializeOutputDest (ToSingleFile path) = writeFile path "" -- truncate
initializeOutputDest (ToManyFiles path) = createDirectoryIfMissing False path

writeTable :: OutputDest
           -> String  -- ^ table title
           -> String  -- ^ contents
           -> IO ()
writeTable (ToManyFiles dir) title contents =
    writeFile (dir </> fname) contents
  where
    fname = map sanitize title
    sanitize ' ' = '-'
    sanitize x = toLower x
writeTable dest title contents =
    write $ unlines
    [ ""
    , "# " <> title
    , ""
    , contents
    ]
  where
    write = case dest of
              ToStdout -> putStrLn
              ToSingleFile path -> appendFile path

main :: IO ()
main = do
    (mode, outputDest, fmt, opts, files) <- execParser $ info (helper <*> args) mempty
    initializeOutputDest outputDest
    let filesWithFriendlyNames :: M.Map MetricFile FilePath
        filesWithFriendlyNames = friendlyNameFiles files
        -- Yuck. Figure out friendly name of reference set
        [ref] = [ name
                | (name, fname) <- M.toList filesWithFriendlyNames
                , fname == head files
                ]

    mtrees <- mapM Ms.readFile filesWithFriendlyNames
        :: IO (M.Map MetricFile (Measurements Double))
    let mtrees' :: M.Map MetricFile (M.Map Label (MeanStddev Double))
        mtrees' = fmap (Ms.toMap (fromJust . meanStdErr)) mtrees
    let aligned :: M.Map Label (M.Map MetricFile (MeanStddev Double))
        aligned = alignMany mtrees'

    let tabulate :: forall rh. (Ord rh, ToRowLabel rh) => String -> LabelMatcher rh -> IO ()
        tabulate heading pred = writeTable outputDest heading contents
          where
            xs :: M.Map rh (M.Map MetricFile (MeanStddev Double))
            xs = M.fromList
               $ mapMaybe (\(k,v) -> (\k' -> (k',v)) <$> match pred k)
               $ M.toList aligned

            contents
              | null xs = "## no samples"
              | otherwise =
                  case mode of
                    RelativeMode ->
                      let showRel = maybe "∙" (showRelative (showGFloat' opts (Just 3)) (showPercent opts))
                          relChg = toRelativeChange ref (fmap (fmap msMean) xs)

                          cols :: [SemiTable Column Cell]
                          cols =
                              [ absoluteCol xs ref, stderrCol xs ref ] ++
                              concat
                              [ [ relativeCol xs ref metricFile
                                , stderrCol xs metricFile
                                ]
                              | metricFile <- M.keys mtrees
                              , metricFile /= ref
                              ]

                          table :: Table rh Column Cell
                          table = foldl' (^|^) (baseTable xs) cols
                       in render fmt id (runShowS . showColumn) id
                          $ mapRowHeader toRowLabel (mapTable (runShowS . showCell opts) table)
                            +====+
                            mapSemiTable (maybe "" (\x -> showsPctChange opts x "")) (geomMeanRow relChg)
                    AbsoluteMode ->
                      let cols :: [SemiTable Column Cell]
                          cols = concat
                              [ [ absoluteCol xs metricFile
                                , stderrCol xs metricFile
                                ]
                              | metricFile <- M.keys mtrees
                              ]

                          table :: Table rh Column Cell
                          table = foldl' (^|^) (baseTable xs) cols
                       in render fmt toRowLabel (runShowS . showColumn) (runShowS . showCell opts) table

    -- compile-time metrics
    tabulate "compiler allocations"  $ objectCompilerRtsStats <* "bytes allocated"
    tabulate "compiler mutator time" $ objectCompilerRtsStats <* ("mutator_cpu_seconds" <|> "mut_cpu_seconds")
    tabulate "compiler GC (cpu) time"      $ objectCompilerRtsStats <* "GC_cpu_seconds"
    tabulate "compiler GC (wall) time"      $ objectCompilerRtsStats <* "GC_wall_seconds"
    tabulate "executable size"       $ testName <* "executable size"
    tabulate "code size (.text)"     $ objectCompilerSize
    -- run-time metrics
    tabulate "bytes allocated"       $ runRtsStats <* "bytes allocated"
    tabulate "mutator time"          $ runRtsStats <* ("mutator_cpu_seconds" <|> "mut_cpu_seconds")
    tabulate "GC (cpu) time"         $ runRtsStats <* "GC_cpu_seconds"
    tabulate "GC (wall) time"        $ runRtsStats <* "GC_wall_seconds"
    tabulate "Elapsed (wall) time"   $ runRtsStats <* "total_wall_seconds"

    -- cachegrind
    tabulate "instructions"          $ cachegrindStats <* "Ir"
    tabulate "LLC cache misses"      $ cachegrindStats <* "DLmr"
    tabulate "L1 cache misses"       $ cachegrindStats <* "D1mr"

    tabulate "perf cache-misses"     $ perfStats <* ("cache-misses" <|> "cache-misses:u")
    tabulate "perf instructions"     $ perfStats <* ("instructions" <|> "instructions:u")
    tabulate "perf cycles"           $ perfStats <* ("cycles" <|> "cycles:u")

alignMany :: (Align f, Ord k) => M.Map k (f a) -> f (M.Map k a)
alignMany mtrees =
    foldl1 (alignWith (mergeThese M.union))
    [ fmap (M.singleton k) mtree
    | (k, mtree) <- M.toList mtrees
    ]

type TestName = String
type ModuleName = String

testName :: LabelMatcher TestName
testName = wildcard

objectCompilerRtsStats :: LabelMatcher (TestName, ModuleName)
objectCompilerRtsStats = (,) <$> testName <* "objects" <*> wildcard <* "rts stats"

objectCompilerSize :: LabelMatcher (TestName,ModuleName)
objectCompilerSize = (,) <$> testName <* "objects" <*> wildcard <* "size"

runRtsStats :: LabelMatcher TestName
runRtsStats = testName <* "run" <* "rts stats"

cachegrindStats :: LabelMatcher TestName
cachegrindStats = testName <* "run" <* "cachegrind"

perfStats :: LabelMatcher TestName
perfStats = testName <* "run" <* "perf"

data Column = AbsoluteCol MetricFile
            | RelativeCol MetricFile
            | StderrCol MetricFile
            deriving (Show)

showColumn :: Column -> ShowS
showColumn (AbsoluteCol mf) = showString $ metricFileFriendlyName mf
showColumn (RelativeCol mf) = showString (metricFileFriendlyName mf) . showString " (rel)"
showColumn (StderrCol mf)   = showString "std. err." --showString (metricFileFriendlyName mf) . showString " (\\sigma)"

data Cell = CellPctChange (ValueBaseline Double)
          | CellMissing
          | CellAbsolute Double
          | CellStderrRel (MeanStddev Double)
          deriving (Show)

showCell :: OutputOptions -> Cell -> ShowS
showCell opts (CellPctChange vb)  = showString $ showPercent opts vb
showCell opts  CellMissing        = showChar '∙'
showCell opts (CellAbsolute x)    = showGFloatAbsolute opts (Just 3) x
showCell opts (CellStderrRel ms)  = showGFloat (Just 1) (msStddev ms / msMean ms * 100) . showChar '%'

baseTable :: M.Map rh (M.Map MetricFile (MeanStddev Double))
          -> Table rh Column Cell
baseTable mtrees =
    Table (Group NoLine $ map Header rowLbls) (Group NoLine []) ([] <$ rowLbls)
  where rowLbls = M.keys mtrees

absoluteCol :: M.Map lbl (M.Map MetricFile (MeanStddev Double))
            -> MetricFile
            -> SemiTable Column Cell
absoluteCol mtrees metricFile =
    col (AbsoluteCol metricFile)
        [ maybe CellMissing (CellAbsolute . msMean) $ M.lookup metricFile xs
        | (_metric, xs) <- M.toList mtrees
        ]

relativeCol :: M.Map lbl (M.Map MetricFile (MeanStddev Double))
            -> MetricFile  -- ^ reference
            -> MetricFile
            -> SemiTable Column Cell
relativeCol mtrees refFile metricFile =
    col (RelativeCol metricFile)
        [ fromMaybe CellMissing $ do
            baseline <- msMean <$> M.lookup refFile xs
            value <- msMean <$> M.lookup metricFile xs
            return $ CellPctChange $ ValueBaseline baseline value
        | (metric, xs) <- M.toList mtrees
        ]

stderrCol :: M.Map lbl (M.Map MetricFile (MeanStddev Double))
          -> MetricFile
          -> SemiTable Column Cell
stderrCol mtrees metricFile =
    col (StderrCol metricFile)
        [ maybe CellMissing CellStderrRel $ M.lookup metricFile xs
        | (_metric, xs) <- M.toList mtrees
        ]

data ValueBaseline a = ValueBaseline { baseline, value :: a }
                     deriving (Show)

showBoth :: (RealFloat a, Show a) => ValueBaseline a -> String
showBoth (ValueBaseline ref val) = show (ref, val)

newtype PctChange a = PctChange a
                    deriving (Show)

showsPctChange :: (Ord a, RealFloat a) => OutputOptions -> PctChange a -> ShowS
showsPctChange opts (PctChange percent) =
    withColor opts color $ prettySign opts rel . showFFloat (Just 2) (abs rel) . showChar '%'
  where
    rel = percent - 100
    color
      | rel < 0 = Green
      | rel > 0 = Red
      | otherwise = White

toPctChange :: RealFloat a => ValueBaseline a -> PctChange a
toPctChange (ValueBaseline ref val) = PctChange ((val / ref) * 100)

showPercent :: (RealFloat a, Show a) => OutputOptions -> ValueBaseline a -> String
showPercent opts x = showsPctChange opts (toPctChange x) ""

data Relative a = Reference a
                | Relative (ValueBaseline a)
                | NoReference

showRelative :: (Ord a, RealFloat a, Show a)
             => (a -> ShowS) -> (ValueBaseline a -> String) -> Relative a -> String
showRelative showRef  _showRel (Reference n) = showRef n ""
showRelative _showRef showRel  (Relative vb) = showRel vb
showRelative _showRef _showRel  NoReference  = "no ref"

toRelativeChange :: (Ord r, Ord c, RealFrac a)
                 => c
                 -> M.Map r (M.Map c a)
                 -> M.Map r (M.Map c (Relative a))
toRelativeChange ref xs = fmap f xs
  where
    f ys
      | Just y0 <- M.lookup ref ys =
          M.mapWithKey (\k y -> if k == ref then Reference y0 else Relative (ValueBaseline y0 y)) ys
      | otherwise = fmap (const NoReference) ys

transpose :: (Ord r, Ord c)
          => M.Map r (M.Map c a)
          -> M.Map c (M.Map r a)
transpose xs = M.fromListWith (<>)
    [ (c, M.singleton r x)
    | (r, cs) <- M.toList xs
    , (c, x) <- M.toList cs
    ]

mean :: (Functor f, Foldable f, RealFloat a)
     => f a -> Maybe a
mean xs
  | null xs   = Nothing
  | otherwise = Just $ (sum xs) / realToFrac (length xs)

data MeanStddev a = MeanStddev { msMean, msStddev :: a }
                  deriving (Show)

meanStddev :: (Functor f, Foldable f, RealFloat a)
       => f a -> Maybe (MeanStddev a)
meanStddev xs = do
    mu <- mean xs
    sigma <- fmap sqrt $ mean $ fmap (\x -> (x-mu)^2) xs
    return $ MeanStddev mu sigma

meanStdErr :: (Functor f, Foldable f, RealFloat a)
       => f a -> Maybe (MeanStddev a)
meanStdErr xs = do
    ms <- meanStddev xs
    return ms { msStddev = msStddev ms / sqrt n }
  where n = realToFrac $ length xs

geomMean :: (Functor f, Foldable f, RealFloat a)
         => f a -> Maybe a
geomMean = fmap exp . mean . fmap log'
  where log' = log . max 0.05

geomMeanRow :: (Ord r, Ord c, RealFloat a)
            => M.Map r (M.Map c (Relative a))
            -> SemiTable String (Maybe (PctChange a))
geomMeanRow xs =
    row "geom mean" (M.elems $ fmap (fmap (PctChange . (*100)) . geomMean . mapMaybe ratios . M.elems) $ transpose xs)
  where
    ratios :: Fractional a => Relative a -> Maybe a
    ratios (Relative (ValueBaseline ref val)) = Just (val / ref)
    ratios _ = Nothing

runShowS :: ShowS -> String
runShowS = ($ [])
