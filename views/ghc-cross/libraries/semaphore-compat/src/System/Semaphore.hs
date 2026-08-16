{-# LANGUAGE BangPatterns #-}
{-# LANGUAGE CPP #-}
{-# LANGUAGE MagicHash #-}
{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE TypeApplications #-}

module System.Semaphore
  ( -- * System semaphores
    Semaphore(..), SemaphoreName(..)
  , createSemaphore, freshSemaphore, openSemaphore
  , waitOnSemaphore, tryWaitOnSemaphore
  , WaitId(..)
  , forkWaitOnSemaphoreInterruptible
  , interruptWaitOnSemaphore
  , getSemaphoreValue
  , releaseSemaphore
  , destroySemaphore

  -- * Abstract semaphores
  , AbstractSem(..)
  , withAbstractSem
  ) where

-- base
import Control.Concurrent
import Control.Monad
import Data.List.NonEmpty ( NonEmpty(..) )
import GHC.Exts ( Char(..), Int(..), indexCharOffAddr# )

-- exceptions
import qualified Control.Monad.Catch as MC

#if defined(mingw32_HOST_OS)
-- Win32
import qualified System.Win32.Event     as Win32
  ( createEvent, setEvent
  , waitForSingleObject, waitForMultipleObjects
  , wAIT_OBJECT_0 )
import qualified System.Win32.File      as Win32
  ( closeHandle )
import qualified System.Win32.Process   as Win32
  ( iNFINITE )
import qualified System.Win32.Semaphore as Win32
  ( Semaphore(..), sEMAPHORE_ALL_ACCESS
  , createSemaphore, openSemaphore, releaseSemaphore )
import qualified System.Win32.Time      as Win32
  ( FILETIME(..), getSystemTimeAsFileTime )
import qualified System.Win32.Types     as Win32
  ( HANDLE, errorWin )
#else
-- base
import Foreign.C.Types
  ( CClock(..) )

-- unix
import qualified System.Posix.Semaphore as Posix
  ( Semaphore, OpenSemFlags(..)
  , semOpen, semWaitInterruptible, semTryWait, semThreadWait
  , semGetValue, semPost, semUnlink )
import qualified System.Posix.Files     as Posix
  ( stdFileMode )
import qualified System.Posix.Process   as Posix
  ( ProcessTimes(systemTime), getProcessTimes )
#endif

---------------------------------------
-- System-specific semaphores

newtype SemaphoreName =
  SemaphoreName { getSemaphoreName :: String }
  deriving Eq

-- | A system semaphore (POSIX or Win32).
data Semaphore =
  Semaphore
    { semaphoreName :: !SemaphoreName
    , semaphore     ::
#if defined(mingw32_HOST_OS)
      !Win32.Semaphore
#else
      !Posix.Semaphore
#endif
    }

-- | Create a new semaphore with the given name and initial amount of
-- available resources.
--
-- Throws an error if a semaphore by this name already exists.
createSemaphore :: SemaphoreName
                -> Int -- ^ number of tokens on the semaphore
                -> IO Semaphore
createSemaphore (SemaphoreName sem_name) init_toks = do
  mb_sem <- create_sem sem_name init_toks
  case mb_sem of
    Left  err -> err
    Right sem -> return sem

-- | Create a fresh semaphore with the given amount of tokens.
--
-- Its name will start with the given prefix, but will have a random suffix
-- appended to it.
freshSemaphore :: String -- ^ prefix
               -> Int    -- ^ number of tokens on the semaphore
               -> IO Semaphore
freshSemaphore prefix init_toks = do
  suffixes <- random_strings
  go 0 suffixes
  where
    go :: Int -> NonEmpty String -> IO Semaphore
    go i (suffix :| suffs) = do
      mb_sem <- create_sem (prefix ++ "_" ++ suffix) init_toks
      case mb_sem of
        Right sem -> return sem
        Left  err
          | next : nexts <- suffs
          , i < 32 -- give up after 32 attempts
          -> go (i+1) (next :| nexts)
          | otherwise
          -> err

create_sem :: String -> Int -> IO (Either (IO Semaphore) Semaphore)
create_sem sem_str init_toks = do
#if defined(mingw32_HOST_OS)
  let toks = fromIntegral init_toks
  mb_sem <- MC.try @_ @MC.SomeException $
    Win32.createSemaphore Nothing toks toks (Just sem_str)
  return $ case mb_sem of
    Right (sem, exists)
      | exists
      -> Left (Win32.errorWin $ "semaphore-compat: semaphore " ++ sem_str ++ " already exists")
      | otherwise
      -> Right $ mk_sem sem
    Left err
      -> Left $ MC.throwM err
#else
  let flags =
        Posix.OpenSemFlags
          { Posix.semCreate    = True
          , Posix.semExclusive = True }
  mb_sem <- MC.try @_ @MC.SomeException $
    Posix.semOpen sem_str flags Posix.stdFileMode init_toks
  return $ case mb_sem of
    Left  err -> Left $ MC.throwM err
    Right sem -> Right $ mk_sem sem
#endif
  where
    sem_nm = SemaphoreName sem_str
    mk_sem sem =
      Semaphore
        { semaphore     = sem
        , semaphoreName = sem_nm }

-- | Open a semaphore with the given name.
--
-- If no such semaphore exists, throws an error.
openSemaphore :: SemaphoreName -> IO Semaphore
openSemaphore nm@(SemaphoreName sem_name) = do
#if defined(mingw32_HOST_OS)
  sem <- Win32.openSemaphore Win32.sEMAPHORE_ALL_ACCESS True sem_name
#else
  let
    flags = Posix.OpenSemFlags
          { Posix.semCreate    = False
          , Posix.semExclusive = False }
  sem <- Posix.semOpen sem_name flags Posix.stdFileMode 0
#endif
  return $
    Semaphore
      { semaphore     = sem
      , semaphoreName = nm }

-- | Indefinitely wait on a semaphore.
--
-- If you want to be able to cancel a wait operation, use
-- 'forkWaitOnSemaphoreInterruptible' instead.
waitOnSemaphore :: Semaphore -> IO ()
waitOnSemaphore (Semaphore { semaphore = sem }) =
#if defined(mingw32_HOST_OS)
  MC.mask_ $ do
    () <$ Win32.waitForSingleObject (Win32.semaphoreHandle sem) Win32.iNFINITE
#else
  Posix.semThreadWait sem
#endif

-- | Try to obtain a token from the semaphore, without blocking.
--
-- Immediately returns 'False' if no resources are available.
tryWaitOnSemaphore :: Semaphore -> IO Bool
tryWaitOnSemaphore (Semaphore { semaphore = sem }) =
#if defined(mingw32_HOST_OS)
  MC.mask_ $ do
    wait_res <- Win32.waitForSingleObject (Win32.semaphoreHandle sem) 0
    return $ wait_res == Win32.wAIT_OBJECT_0
#else
  Posix.semTryWait sem
#endif

-- | Release a semaphore: add @n@ to its internal counter.
--
-- No-op when `n <= 0`.
releaseSemaphore :: Semaphore -> Int -> IO ()
releaseSemaphore (Semaphore { semaphore = sem }) n
  | n <= 0
  = return ()
  | otherwise
  = MC.mask_ $ do
#if defined(mingw32_HOST_OS)
    void $ Win32.releaseSemaphore sem (fromIntegral n)
#else
    replicateM_ n (Posix.semPost sem)
#endif

-- | Destroy the given semaphore.
destroySemaphore :: Semaphore -> IO ()
destroySemaphore sem =
#if defined(mingw32_HOST_OS)
  Win32.closeHandle (Win32.semaphoreHandle $ semaphore sem)
#else
  Posix.semUnlink (getSemaphoreName $ semaphoreName sem)
#endif

-- | Query the current semaphore value (how many tokens it has available).
--
-- This is mainly for debugging use, as it is easy to introduce race conditions
-- when nontrivial program logic depends on the value returned by this function.
getSemaphoreValue :: Semaphore -> IO Int
getSemaphoreValue (Semaphore { semaphore = sem }) =
#if defined(mingw32_HOST_OS)
  MC.mask_ $ do
    wait_res <- Win32.waitForSingleObject (Win32.semaphoreHandle sem) 0
    if wait_res == Win32.wAIT_OBJECT_0
      -- We were able to acquire a resource from the semaphore without waiting:
      -- release it immediately, thus obtaining the total number of available
      -- resources.
    then
      (+1) . fromIntegral <$> Win32.releaseSemaphore sem 1
    else
      return 0
#else
  Posix.semGetValue sem
#endif

-- | 'WaitId' stores the information we need to cancel a thread
-- which is waiting on a semaphore.
--
-- See 'forkWaitOnSemaphoreInterruptible' and 'interruptWaitOnSemaphore'.
data WaitId = WaitId { waitingThreadId :: ThreadId
#if defined(mingw32_HOST_OS)
                     , cancelHandle    :: Win32.HANDLE
#endif
                     }

-- | Spawn a thread that waits on the given semaphore.
--
-- In this thread, asynchronous exceptions will be masked.
--
-- The waiting operation can be interrupted using the
-- 'interruptWaitOnSemaphore' function.
--
-- This implements a similar pattern to the @forkFinally@ function.
forkWaitOnSemaphoreInterruptible
  :: Semaphore
  -> ( Either MC.SomeException Bool -> IO () ) -- ^ wait result action
  -> IO WaitId
forkWaitOnSemaphoreInterruptible
  (Semaphore { semaphore = sem })
  wait_result_action = do
#if defined(mingw32_HOST_OS)
    cancelHandle <- Win32.createEvent Nothing True False ""
#endif
    let
      interruptible_wait :: IO Bool
      interruptible_wait =
#if defined(mingw32_HOST_OS)
        -- Windows: wait on both the handle used for cancelling the wait
        -- and on the semaphore.
          do
            wait_res <-
              Win32.waitForMultipleObjects
                [ Win32.semaphoreHandle sem
                , cancelHandle ]
                False -- False <=> WaitAny
                Win32.iNFINITE
            return $ wait_res == Win32.wAIT_OBJECT_0
            -- Only in the case that the wait result is WAIT_OBJECT_0 will
            -- we have succeeded in obtaining a token from the semaphore.
#else
        -- POSIX: use the 'semWaitInterruptible' interruptible FFI call
        -- that can be interrupted when we send a killThread signal.
          Posix.semWaitInterruptible sem
#endif
    waitingThreadId <- forkIO $ MC.mask_ $ do
      wait_res <- MC.try interruptible_wait
      wait_result_action wait_res
    return $ WaitId { .. }

-- | Interrupt a semaphore wait operation initiated by
-- 'forkWaitOnSemaphoreInterruptible'.
interruptWaitOnSemaphore :: WaitId -> IO ()
interruptWaitOnSemaphore ( WaitId { .. } ) = do
#if defined(mingw32_HOST_OS)
  Win32.setEvent cancelHandle
    -- On Windows, we signal to stop waiting.
#endif
  killThread waitingThreadId
    -- On POSIX, killing the thread will cancel the wait on the semaphore
    -- due to the FFI call being interruptible ('semWaitInterruptible').

---------------------------------------
-- Abstract semaphores

-- | Abstraction over the operations of a semaphore.
data AbstractSem =
  AbstractSem
    { acquireSem :: IO ()
    , releaseSem :: IO ()
    }

withAbstractSem :: AbstractSem -> IO b -> IO b
withAbstractSem sem = MC.bracket_ (acquireSem sem) (releaseSem sem)

---------------------------------------
-- Utility

iToBase62 :: Int -> String
iToBase62 m = go m' ""
  where
    m'
      | m == minBound
      = maxBound
      | otherwise
      = abs m
    go n cs | n < 62
            = let !c = chooseChar62 n
              in c : cs
            | otherwise
            = let !(!q, r) = quotRem n 62
                  !c       = chooseChar62 r
              in go q (c : cs)

    chooseChar62 :: Int -> Char
    {-# INLINE chooseChar62 #-}
    chooseChar62 (I# n) = C# (indexCharOffAddr# chars62 n)
    chars62 = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"#

random_strings :: IO (NonEmpty String)
random_strings = do
#if defined(mingw32_HOST_OS)
  Win32.FILETIME t <- Win32.getSystemTimeAsFileTime
#else
  CClock t <- Posix.systemTime <$> Posix.getProcessTimes
#endif
  return $ fmap ( \ i -> iToBase62 (i + fromIntegral t) ) (0 :| [1..])
