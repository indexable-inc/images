import           Control.DeepSeq                       (force)
import           Data.Foldable                         (foldMap)
import           Data.Maybe                            (listToMaybe, fromJust)
import           Data.Monoid
import           Data.String
import           Prelude                               hiding (words, head, tail)

import           Test.Tasty.Bench
import           Data.ByteString.Builder
import           Data.ByteString.Builder.Extra         (byteStringCopy, byteStringInsert, intHost)
import           Data.ByteString.Builder.Internal      (ensureFree)
import           Data.ByteString.Builder.Prim          (BoundedPrim, FixedPrim, (>$<))
import qualified Data.ByteString.Builder.Prim          as P
import qualified Data.ByteString.Builder.Prim.Internal as PI

import           Foreign

import System.Random
import Data.Bifunctor (first)

------------------------------------------------------------------------------
-- Benchmark
------------------------------------------------------------------------------

-- input data (NOINLINE to ensure memoization)
----------------------------------------------

-- | Few-enough repetitions to avoid making GC too expensive.
nRepl :: Int
nRepl = 10000

{-# NOINLINE intData #-}
intData :: [Int]
intData = [1..nRepl]

{-# NOINLINE byteStringData #-}
byteStringData :: S.OS_STRING
byteStringData = S.pack $ map w intData

{-# NOINLINE loremIpsum #-}
loremIpsum :: S.OS_STRING
loremIpsum = [OSSTR|incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis
nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.
Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu
fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in
culpa qui officia deserunt mollit anim id est laborum.|]

-- benchmark wrappers
---------------------

{-# INLINE benchB' #-}
benchB'
  :: String -> a -> (a -> OS_STRING) -> Benchmark
benchB' name x b = bench name $ whnf (S.length . b) x


-- We use this construction of just looping through @n,n-1,..,1@ to ensure that
-- we measure the speed of the encoding and not the speed of generating the
-- values to be encoded.
{-# INLINE benchIntEncodingB #-}
benchIntEncodingB :: Int              -- ^ Maximal 'Int' to write
                  -> BoundedPrim Int  -- ^ 'BoundedPrim' to execute
                  -> IO ()            -- ^ 'IO' action to benchmark
benchIntEncodingB n0 w
  | n0 <= 0   = return ()
  | otherwise = do
      fpbuf <- mallocForeignPtrBytes (n0 * PI.sizeBound w)
      withForeignPtr fpbuf (loop n0) >> return ()
  where
    loop !n !op
      | n <= 0    = return op
      | otherwise = PI.runB w n op >>= loop (n - 1)


-- Helpers
-------------

hashInt :: Int -> Int
hashInt x = iterate step x !! 10
  where
    step a = e
      where b = (a `xor` 61) `xor` (a `shiftR` 16)
            c = b + (b `shiftL` 3)
            d = c `xor` (c `shiftR` 4)
            e = d * 0x27d4eb2d
            f = e `xor` (e `shiftR` 15)


foldInputs'
  :: [[ OS_CHAR ]]
foldInputs' = force (S.unpack <$> foldInputs)

foldInputs :: [S.OS_STRING]
foldInputs = map (\k -> S.pack . map w $ if k <= 6 then take (2 ^ k) [32..95] else concat (replicate (2 ^ (k - 6)) [32..95])) [0..16]

largeTraversalInput :: S.OS_STRING
largeTraversalInput = S.concat (replicate 10 byteStringData)

smallTraversalInput :: S.OS_STRING
smallTraversalInput = [OSSTR|The quick brown fox|]

zeroes :: S.OS_STRING
zeroes = S.replicate 10000 (w 0)

partitionStrict p = nf (S.partition p) . randomStrict $ mkStdGen 98423098
  where randomStrict = fst . S.unfoldrN 10000 (Just . first S.unsafeFromChar . random)

-- ASCII \n to ensure no typos
nl :: OS_CHAR
nl = w 0xa
{-# INLINE nl #-}

-- non-inlined equality test
nilEq :: OS_CHAR -> OS_CHAR -> Bool
{-# NOINLINE nilEq #-}
nilEq = (==)

-- lines of 200 letters from a to e, followed by repeated letter f
absurdlong :: S.OS_STRING
absurdlong = (S.replicate 200 (w 0x61) <> S.singleton nl
          <> S.replicate 200  (w 0x62) <> S.singleton nl
          <> S.replicate 200  (w 0x63) <> S.singleton nl
          <> S.replicate 200  (w 0x64) <> S.singleton nl
          <> S.replicate 200  (w 0x65) <> S.singleton nl)
          <> S.replicate 999999 (w 0x66)

bench_find_index_second :: OS_STRING -> Maybe Int
bench_find_index_second bs =
  let isNl = (== nl)
   in case S.findIndex isNl bs of
        Just !i -> S.findIndex isNl (S.drop (i+1) bs)
        Nothing -> Nothing
{-# INLINE bench_find_index_second #-}

bench_elem_index_second :: OS_STRING -> Maybe Int
bench_elem_index_second bs =
    case S.elemIndex nl bs of
        Just !i -> S.elemIndex nl (S.drop (i+1) bs)
        Nothing -> Nothing
{-# INLINE bench_elem_index_second #-}



-- benchmarks
-------------

benchMark :: Benchmark
benchMark = absurdlong `seq` bgroup benchStr
    [ bgroup "Small payload"
      [ benchB' "mempty"        ()  (const mempty)
      , benchB' "UTF-8 String (naive)" "hello world\0" (fromJust . S.encodeUtf)
      , benchB' "String (naive)" "hello world!" (fromJust . S.encodeUtf)
      ]
    , bgroup "intercalate"
      [ bench "intercalate (large)" $ whnf (S.intercalate $ [OSSTR| and also |]) (replicate 300 [OSSTR|expression|])
      , bench "intercalate (small)" $ whnf (S.intercalate [OSSTR|&|]) (replicate 30 [OSSTR|foo|])
      , bench "intercalate (tiny)" $ whnf (S.intercalate [OSSTR|&|]) [[OSSTR|foo|], [OSSTR|bar|], [OSSTR|baz|]]
      ]
    , bgroup "partition"
      [
        bgroup "strict"
        [
          bench "mostlyTrueFast"  $ partitionStrict (< (w 225))
        , bench "mostlyFalseFast" $ partitionStrict (< (w 10))
        , bench "balancedFast"    $ partitionStrict (< (w 128))

        , bench "mostlyTrueSlow"  $ partitionStrict (\x -> hashWord8 x < w 225)
        , bench "mostlyFalseSlow" $ partitionStrict (\x -> hashWord8 x < w 10)
        , bench "balancedSlow"    $ partitionStrict (\x -> hashWord8 x < w 128)
        ]
      ]
    , bgroup "folds"
      [ bgroup "strict"
        [ bgroup "foldl" $ map (\s -> bench (show $ S.length s) $
            nf (S.foldl (\acc x -> acc +  iw x) (0 :: Int)) s) foldInputs
        , bgroup "foldl'" $ map (\s -> bench (show $ S.length s) $
            nf (S.foldl' (\acc x -> acc + iw x) (0 :: Int)) s) foldInputs
        , bgroup "foldr" $ map (\s -> bench (show $ S.length s) $
            nf (S.foldr (\x acc -> iw x + acc) (0 :: Int)) s) foldInputs
        , bgroup "foldr'" $ map (\s -> bench (show $ S.length s) $
            nf (S.foldr' (\x acc -> iw x + acc) (0 :: Int)) s) foldInputs
        , bgroup "foldr1'" $ map (\s -> bench (show $ S.length s) $
            nf (S.foldr1' (\x acc -> w $ iw x + iw acc)) s) foldInputs
        , bgroup "unfoldrN" $ map (\s -> bench (show $ S.length s) $
            nf (S.unfoldrN (S.length s) (\a -> Just (w a, a + 1))) 0) foldInputs
        , bgroup "filter" $ map (\s -> bench (show $ S.length s) $
            nf (S.filter (odd . iw)) s) foldInputs
        ]
      ]
    , bgroup "findIndexOrLength"
      [ bench "takeWhile"      $ nf (S.takeWhile (even . iw)) zeroes
      , bench "dropWhile"      $ nf (S.dropWhile (even . iw)) zeroes
      , bench "break"          $ nf (S.break (odd . iw)) zeroes
      ]
    , bgroup "findIndex_"
      [ bench "findIndices"    $ nf (sum . S.findIndices (\x -> x ==  w 129 || x == w 72)) byteStringData
      , bench "find"           $ nf (S.find (>= w 198)) byteStringData
      ]
    , bgroup "traversals"
      [ bench "map (+1) large" $ nf (S.map (w . (+ 1) . iw)) largeTraversalInput
      , bench "map (+1) small" $ nf (S.map (w . (+ 1) . iw)) smallTraversalInput
      ]
    , bgroup (benchStr <> " strict first index") $
        [ bench "FindIndices" $ nf (listToMaybe . S.findIndices (== nl)) absurdlong
        , bench "ElemIndices" $ nf (listToMaybe . S.elemIndices     nl)  absurdlong
        , bench "FindIndex"   $ nf (S.findIndex (== nl)) absurdlong
        , bench "ElemIndex"   $ nf (S.elemIndex     nl)  absurdlong
        ]
    , bgroup (benchStr <> " strict second index") $
        [ bench "FindIndices" $ nf (listToMaybe . drop 1 . S.findIndices (== nl)) absurdlong
        , bench "ElemIndices" $ nf (listToMaybe . drop 1 . S.elemIndices     nl)  absurdlong
        , bench "FindIndex"   $ nf bench_find_index_second absurdlong
        , bench "ElemIndex"   $ nf bench_elem_index_second absurdlong
        ]
    , bgroup (benchStr <> " index equality inlining") $
        [ bench "FindIndices/inlined"     $ nf (S.findIndices    (== nl)) absurdlong
        , bench "FindIndices/non-inlined" $ nf (S.findIndices (nilEq nl)) absurdlong
        , bench "FindIndex/inlined"       $ nf (S.findIndex      (== nl)) absurdlong
        , bench "FindIndex/non-inlined"   $ nf (S.findIndex   (nilEq nl)) absurdlong
        ]
    , bgroup (benchStr <> " conversions") $
        [ bgroup "unpack" $ map (\s -> bench (show $ S.length s) $
            nf (\x -> S.unpack x) s) foldInputs
        , bgroup "pack" $ map (\s -> bench (show $ length s) $
            nf S.pack s) foldInputs'
        , bench "unpack and get last element" $ nf (\x -> last . S.unpack $ x) absurdlong
        , bench "unpack and get first 120 elements" $ nf (\x -> take 120 . S.unpack $ x) absurdlong
        ]
    ]

