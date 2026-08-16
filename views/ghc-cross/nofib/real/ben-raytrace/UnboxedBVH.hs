{-# LANGUAGE MagicHash #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE GeneralizedNewtypeDeriving #-}
{-# LANGUAGE TypeInType #-}
{-# LANGUAGE UnboxedTuples #-}
{-# LANGUAGE BangPatterns #-}

module UnboxedBVH
  ( BVH, packBVH
  , bvhFigure
  ) where

import Data.Primitive.ByteArray
import Data.Primitive.Array
import Data.Primitive.Types
import Data.Word
import Control.Applicative
import Control.Monad.Trans.State.Strict
import Control.Monad.Trans.Class
import Control.Monad.ST
import Control.Monad

import Figure
import BoundingBox
import Interval
import Ray
import Vector
import qualified BVH as B

data BVH
    = BVH { nodeArr :: !ByteArray
          , leafArr :: !(Array Figure)
          }

-- Layout
-- (all scalars are single-precision).
--
-- index           datum
-- ---------       ----------
-- 8*n+0           boxAX   :: Float
-- 8*n+1           boxAY   :: Float
-- 8*n+2           boxAZ   :: Float
-- 8*n+3           boxBX   :: Float
-- 8*n+4           boxBY   :: Float
-- 8*n+5           boxBZ   :: Float
-- 8*n+6           childA  :: Word32
-- 8*n+7           childB  :: Word32
--
-- If childA == 0 then node is a leaf
-- with figure at leafArr[childB]

packBVH :: B.BVH -> BVH
packBVH bvh = runST $ do
    let leafCount = bvhLeafCount bvh
    let nodeCount = bvhNodeCount bvh
    leafArr <- newArray leafCount undefined
    nodeArr <- newByteArray (8*8*(nodeCount + leafCount))
    _ <- packBVH' nodeArr leafArr bvh
    leafArr <- unsafeFreezeArray leafArr
    nodeArr <- unsafeFreezeByteArray nodeArr
    return $ BVH {..}

type BuildM s = StateT Indexes (ST s)

data Entry
    = NodeEntry { nodeBoundingBox :: BoundingBox
                , nodeChildA, nodeChildB :: NodeIdx
                }
    | LeafEntry { nodeBoundingBox :: BoundingBox
                , nodeLeafIdx :: LeafIdx
                }

writeNode :: forall s. MutableByteArray s -> NodeIdx -> Entry -> ST s ()
writeNode nodeArr nodeIdx ent = do
    writeFloat 0 $ nudge (-1) (vec3X boxA)
    writeFloat 1 $ nudge (-1) (vec3Y boxA)
    writeFloat 2 $ nudge (-1) (vec3Z boxA)
    writeFloat 3 $ nudge ( 1) (vec3X boxB)
    writeFloat 4 $ nudge ( 1) (vec3Y boxB)
    writeFloat 5 $ nudge ( 1) (vec3Z boxB)
    case ent of
        NodeEntry {..} -> do
            writeNodeIdx 6 nodeChildA
            writeNodeIdx 7 nodeChildB
        LeafEntry _bbox (LeafIdx leafIdx) -> do
            writeNodeIdx 6 (NodeIdx 0)
            writeNodeIdx 7 (NodeIdx leafIdx)
  where
    base = nodeIdxBase nodeIdx
    bbox = nodeBoundingBox ent
    Pt boxA = bbMin bbox
    Pt boxB = bbMax bbox

    nudge :: Double -> Double -> Float
    nudge m x = realToFrac $ x * (1 + m*delta)
      where delta = 1e-5

    writeNodeIdx :: Int -> NodeIdx -> ST s ()
    writeNodeIdx off x = writeByteArray nodeArr (base + off) x

    writeFloat :: Int -> Float -> ST s ()
    writeFloat off x = writeByteArray nodeArr (base + off) x

    toFloat :: Double -> Float
    toFloat = realToFrac -- TODO: rounding direction

readNode :: ByteArray -> NodeIdx -> Entry
readNode nodeArr nodeIdx
  | NodeIdx 0 <- childA =
      LeafEntry bbox (LeafIdx $ case childB of NodeIdx n -> n)
  | otherwise =
      NodeEntry bbox childA childB
  where
    base = nodeIdxBase nodeIdx

    childA = readNodeIdx 6
    childB = readNodeIdx 7

    bbox :: BoundingBox
    bbox = mkBoundingBox (Pt bbMin) (Pt bbMax)

    bbMin, bbMax :: Vec3
    bbMin = Vec3 (readFloat 0) (readFloat 1) (readFloat 2)
    bbMax = Vec3 (readFloat 3) (readFloat 4) (readFloat 5)

    readNodeIdx :: Int -> NodeIdx
    readNodeIdx off = indexByteArray nodeArr (base + off)

    readFloat :: Int -> Double
    readFloat off =
        let n :: Float
            n = indexByteArray nodeArr (base + off)
        in realToFrac n

packBVH' :: forall s. MutableByteArray s -> MutableArray s Figure -> B.BVH -> ST s ()
packBVH' nodeArr leafArr = \bvh -> void $ runStateT (go bvh) (Indexes (LeafIdx 0) (NodeIdx 0))
  where
    go :: B.BVH -> BuildM s NodeIdx
    go (B.BVHNode bbox childA childB) = do
        nodeIdx <- getNodeIdx
        idxA <- go childA
        idxB <- go childB
        lift $ writeNode nodeArr nodeIdx $ NodeEntry bbox idxA idxB
        return nodeIdx

    go (B.BVHLeaf figure) = do
        nodeIdx <- getNodeIdx
        leafIdx <- getLeafIdx
        lift $ writeNode nodeArr nodeIdx $ LeafEntry (boundingBox figure) leafIdx
        writeArray leafArr (case leafIdx of LeafIdx n -> fromIntegral n) figure
        return nodeIdx

    getLeafIdx :: BuildM s LeafIdx
    getLeafIdx = do
        Indexes leafIdx nodeIdx <- get
        put $! Indexes (succ leafIdx) nodeIdx
        return leafIdx

    getNodeIdx :: BuildM s NodeIdx
    getNodeIdx = do
        Indexes leafIdx nodeIdx <- get
        put $! Indexes leafIdx (succ nodeIdx)
        return nodeIdx

hitBVH :: Ray3 -> Interval -> BVH -> Maybe Hit
hitBVH ray hitInterval0 (BVH {..}) = go (NodeIdx 0) hitInterval0
  where
    go :: NodeIdx -> Interval -> Maybe Hit
    go nodeIdx !hitInterval = do
        let node = readNode nodeArr nodeIdx
        guard $ boundingBoxHits (nodeBoundingBox node) ray
        case node of
            LeafEntry _bbox (LeafIdx leafIdx) ->
                let figure = indexArray leafArr (fromIntegral leafIdx)
                 in hitTest figure ray hitInterval
            NodeEntry _bbox childA childB ->
                goChildren childA childB hitInterval

    goChildren :: NodeIdx -> NodeIdx -> Interval -> Maybe Hit
    goChildren childA childB hitInterval = do
        let hitA = go childA hitInterval
            hitInterval' = case hitA of
              Just ha -> hitInterval { iUpper = hitDistance ha }
              Nothing -> hitInterval
        let hitB = go childB hitInterval'
        hitB <|> hitA

newtype LeafIdx
   = LeafIdx {getLeafIdx :: Word32}
   deriving (Enum, Prim)

newtype NodeIdx
   = NodeIdx {getNodeIdx :: Word32}
   deriving (Enum, Prim)

-- | Base index (in 32-bit units) of given node in 'nodeArr'.
nodeIdxBase :: NodeIdx -> Int
nodeIdxBase (NodeIdx n) = fromIntegral n * 8

data Indexes = Indexes !LeafIdx !NodeIdx

bvhNodeCount :: B.BVH -> Int
bvhNodeCount = go
  where
    go (B.BVHNode _ a b) = 1 + go a + go b
    go (B.BVHLeaf _) = 0

bvhLeafCount :: B.BVH -> Int
bvhLeafCount = go
  where
    go (B.BVHNode _ a b) = go a + go b
    go (B.BVHLeaf _) = 1

bvhFigure :: BVH -> Figure
bvhFigure bvh = Figure {..}
  where
    boundingBox = nodeBoundingBox $ readNode (nodeArr bvh) (NodeIdx 0)
    hitTest ray hitInterval = hitBVH ray hitInterval bvh
