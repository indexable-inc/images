module System.OsString.Internal.Exception where

import Control.Exception ( catch, fromException, toException, throwIO, Exception, SomeAsyncException(..) )

-- | Like 'try', but rethrows async exceptions.
trySafe :: Exception e => IO a -> IO (Either e a)
trySafe ioA = catch action eHandler
 where
  action = do
    v <- ioA
    return (Right v)
  eHandler e
    | isAsyncException e = throwIO e
    | otherwise = return (Left e)

isAsyncException :: Exception e => e -> Bool
isAsyncException e =
    case fromException (toException e) of
        Just (SomeAsyncException _) -> True
        Nothing -> False
