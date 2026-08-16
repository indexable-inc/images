{-# LANGUAGE DeriveFunctor #-}
{-# LANGUAGE GADTs #-}

module LabelMatch where

import Control.Applicative
import Control.Monad
import Data.String
import qualified Data.Text as T

import Measurements (Label(..))

newtype LabelMatcher a = LabelMatcher (Label -> Maybe (Label, a))
    deriving (Functor)

match :: LabelMatcher a -> Label -> Maybe a
match (LabelMatcher f) l = snd <$> f l

instance (a ~ ()) => IsString (LabelMatcher a) where
    fromString = matchPart . fromString

instance Applicative LabelMatcher where
    pure x = LabelMatcher $ \lbl -> Just (lbl, x)
    (<*>) = ap

instance Alternative LabelMatcher where
    LabelMatcher f <|> LabelMatcher g =
      LabelMatcher $ \lbl -> f lbl <|> g lbl
    empty = LabelMatcher $ const Nothing

instance Monad LabelMatcher where
    LabelMatcher f >>= g = LabelMatcher $ \lbl ->
        case f lbl of
          Nothing -> Nothing
          Just (lbl', x) -> let LabelMatcher h = g x
                             in h lbl'

matchPart :: T.Text -> LabelMatcher ()
matchPart s = LabelMatcher f
  where
    f (Label (x:xs))
      | x == s = Just (Label xs, ())
    f _ = Nothing

wildcard :: LabelMatcher String
wildcard = LabelMatcher f
  where
    f (Label (x:xs)) = Just (Label xs, T.unpack x)
    f (Label []) = Nothing

end :: LabelMatcher ()
end = LabelMatcher f
  where
    f (Label []) = Just (Label [], ())
    f (Label _) = Nothing

