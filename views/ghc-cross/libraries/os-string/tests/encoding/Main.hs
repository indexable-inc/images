{-# LANGUAGE TypeApplications #-}

module Main (main) where

import qualified EncodingSpec as Spec
import TestUtil

main :: IO ()
main = runTests (Spec.tests)

