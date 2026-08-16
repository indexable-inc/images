{-# LANGUAGE CPP #-}
#if __GLASGOW_HASKELL__ >= 702
{-# LANGUAGE Safe #-}
#endif
#if __GLASGOW_HASKELL__ >= 710 && __GLASGOW_HASKELL__ < 802
{-# LANGUAGE AutoDeriveTypeable #-}
#endif
#if __GLASGOW_HASKELL__ >= 806
{-# LANGUAGE QuantifiedConstraints #-}
#endif
-----------------------------------------------------------------------------
-- |
-- Module      :  Control.Monad.Trans.Class
-- Copyright   :  (c) Andy Gill 2001,
--                (c) Oregon Graduate Institute of Science and Technology, 2001
-- License     :  BSD-style (see the file LICENSE)
--
-- Maintainer  :  R.Paterson@city.ac.uk
-- Stability   :  experimental
-- Portability :  portable
--
-- The class of monad transformers.
--
-- A monad transformer makes a new monad out of an existing monad, such
-- that computations of the old monad may be embedded in the new one.
-- To construct a monad with a desired set of features, one typically
-- starts with a base monad, such as 'Data.Functor.Identity.Identity', @[]@ or 'IO', and
-- applies a sequence of monad transformers.
-----------------------------------------------------------------------------

module Control.Monad.Trans.Class (
    -- * Transformer class
    MonadTrans(..)

    -- * Conventions
    -- $conventions

    -- * Strict monads
    -- $strict

    -- * Examples
    -- ** Parsing
    -- $example1

    -- ** Parsing and counting
    -- $example2

    -- ** Interpreter monad
    -- $example3
  ) where

-- | The class of monad transformers.
-- For any monad @m@, the result @t m@ should also be a monad,
-- and 'lift' should be a monad transformation from @m@ to @t m@,
-- i.e. it should satisfy the following laws:
--
-- * @'lift' . 'return' = 'return'@
--
-- * @'lift' (m >>= f) = 'lift' m >>= ('lift' . f)@
--
-- Since 0.6.0.0 and for GHC 8.6 and later, the requirement that @t m@
-- be a 'Monad' is enforced by the implication constraint
-- @forall m. 'Monad' m => 'Monad' (t m)@ enabled by the
-- @QuantifiedConstraints@ extension.
--
-- === __Ambiguity error with GHC 9.0 to 9.2.2__
-- These versions of GHC have a bug
-- (<https://gitlab.haskell.org/ghc/ghc/-/issues/20582>)
-- which causes constraints like
--
-- @
-- (MonadTrans t, forall m. Monad m => Monad (t m)) => ...
-- @
--
-- to be reported as ambiguous.  For transformers 0.6 and later, this can
-- be fixed by removing the second constraint, which is implied by the first.
#if __GLASGOW_HASKELL__ >= 806
class (forall m. Monad m => Monad (t m)) => MonadTrans t where
#else
-- Prior to GHC 8.8 (base-4.13), the Monad class included fail.
-- GHC 8.6 (base-4.12) has MonadFailDesugaring on by default, so there
-- is no need for users defining monad transformers to define fail in
-- the Monad instance of the transformed monad.
class MonadTrans t where
#endif
    -- | Lift a computation from the argument monad to the constructed monad.
    lift :: (Monad m) => m a -> t m a

{- $conventions
All monad transformer modules except 'Control.Monad.Trans.Maybe'
include the special case of applying the transformer
to 'Data.Functor.Identity.Identity'.  For example,
@'Control.Monad.Trans.State.Lazy.State' s@ is an abbreviation for
@'Control.Monad.Trans.State.Lazy.StateT' s 'Data.Functor.Identity.Identity'@.
As a consequence, operations defined on the monad transformer can also
be used on this special case.

Each monad transformer also comes with an operation @run@/XXX/@T@ to
unwrap the transformer, exposing a computation of the inner monad.
(Currently these functions are defined as field labels, but in a future
major release they may be separate functions.)

All of the monad transformers except 'Control.Monad.Trans.Cont.ContT'
and 'Control.Monad.Trans.Cont.SelectT' are functors on the category
of monads: in addition to defining a mapping of monads, they
also define a mapping from transformations between base monads to
transformations between transformed monads, called @map@/XXX/@T@.
Thus given a monad transformation @t :: M a -> N a@, the combinator
'Control.Monad.Trans.State.Lazy.mapStateT' constructs a monad
transformation

> mapStateT t :: StateT s M a -> StateT s N a

For these monad transformers, 'lift' is a natural transformation in the
category of monads, i.e. for any monad transformation @t :: M a -> N a@,

* @map@/XXX/@T t . 'lift' = 'lift' . t@

Each of the monad transformers introduces relevant operations.
In a sequence of monad transformers, most of these operations.can be
lifted through other transformers using 'lift' or the @map@/XXX/@T@
combinator, but a few with more complex type signatures require
specialized lifting combinators, called @lift@/Op/
(see "Control.Monad.Signatures").
-}

{- $strict

A monad is said to be /strict/ if its '>>=' operation (and therefore also
'>>') is strict in its first argument.  The base monads 'Maybe', @[]@
and 'IO' are strict:

>>> undefined >> Just 2
*** Exception: Prelude.undefined
>>> undefined >> [2]
*** Exception: Prelude.undefined
>>> undefined >> print 2
*** Exception: Prelude.undefined

However, the monads 'Data.Functor.Identity.Identity' and @(->) a@ are not:

>>> undefined >> Identity 2
Identity 2
>>> (undefined >> (+1)) 5
6

In a strict monad you know when each action is executed, but the monad
is not necessarily strict in the return value, or in other components
of the monad, such as a state.  However, you can use 'seq' to create
an action that is strict in the component you want evaluated.
-}

{- $example1

The first example is a parser monad in the style of

* \"Monadic parsing in Haskell\", by Graham Hutton and Erik Meijer,
/Journal of Functional Programming/ 8(4):437-444, July 1998
(<http://www.cs.nott.ac.uk/~pszgmh/bib.html#pearl>).

We can define such a parser monad by adding a state (the 'String' remaining
to be parsed) to the @[]@ monad, which provides non-determinism:

> import Control.Monad.Trans.State
>
> type Parser = StateT String []

Then @Parser@ is an instance of @MonadPlus@: monadic sequencing implements
concatenation of parsers, while @mplus@ provides choice.  To use parsers,
we need a primitive to run a constructed parser on an input string:

> runParser :: Parser a -> String -> [a]
> runParser p s = [x | (x, "") <- runStateT p s]

Finally, we need a primitive parser that matches a single character,
from which arbitrarily complex parsers may be constructed:

> item :: Parser Char
> item = do
>     c:cs <- get
>     put cs
>     return c

In this example we use the operations @get@ and @put@ from
"Control.Monad.Trans.State", which are defined only for monads that are
applications of 'Control.Monad.Trans.State.Lazy.StateT'.  Alternatively one
could use monad classes from the @mtl@ package or similar, which contain
methods @get@ and @put@ with types generalized over all suitable monads.
-}

{- $example2

We can define a parser that also counts by adding a
'Control.Monad.Trans.Writer.Lazy.WriterT' transformer:

> import Control.Monad.Trans.Class
> import Control.Monad.Trans.State
> import Control.Monad.Trans.Writer
> import Data.Monoid
>
> type Parser = WriterT (Sum Int) (StateT String [])

The function that applies a parser must now unwrap each of the monad
transformers in turn:

> runParser :: Parser a -> String -> [(a, Int)]
> runParser p s = [(x, n) | ((x, Sum n), "") <- runStateT (runWriterT p) s]

To define the @item@ parser, we need to lift the
'Control.Monad.Trans.State.Lazy.StateT' operations through the
'Control.Monad.Trans.Writer.Lazy.WriterT' transformer.

> item :: Parser Char
> item = do
>     c:cs <- lift get
>     lift (put cs)
>     return c

In this case, we were able to do this with 'lift', but operations with
more complex types require special lifting functions, which are provided
by monad transformers for which they can be implemented.  If you use the
monad classes of the @mtl@ package or similar, this lifting is handled
automatically by the instances of the classes, and you need only use
the generalized methods @get@ and @put@.

We can also define a primitive using the Writer:

> tick :: Parser ()
> tick = tell (Sum 1)

Then the parser will keep track of how many @tick@s it executes.
-}

{- $example3

This example is a cut-down version of the one in

* \"Monad Transformers and Modular Interpreters\",
by Sheng Liang, Paul Hudak and Mark Jones in /POPL'95/
(<http://web.cecs.pdx.edu/~mpj/pubs/modinterp.html>).

Suppose we want to define an interpreter that can do I\/O and has
exceptions, an environment and a modifiable store.  We can define
a monad that supports all these things as a stack of monad transformers:

> import Control.Monad.Trans.Class
> import Control.Monad.Trans.State
> import qualified Control.Monad.Trans.Reader as R
> import qualified Control.Monad.Trans.Except as E
> import Control.Monad.IO.Class
>
> type InterpM = StateT Store (R.ReaderT Env (E.ExceptT Err IO))

for suitable types @Store@, @Env@ and @Err@.

Now we would like to be able to use the operations associated with each
of those monad transformers on @InterpM@ actions.  Since the uppermost
monad transformer of @InterpM@ is 'Control.Monad.Trans.State.Lazy.StateT',
it already has the state operations @get@ and @set@.

The first of the 'Control.Monad.Trans.Reader.ReaderT' operations,
'Control.Monad.Trans.Reader.ask', is a simple action, so we can lift it
through 'Control.Monad.Trans.State.Lazy.StateT' to @InterpM@ using 'lift':

> ask :: InterpM Env
> ask = lift R.ask

The other 'Control.Monad.Trans.Reader.ReaderT' operation,
'Control.Monad.Trans.Reader.local', has a suitable type for lifting
using 'Control.Monad.Trans.State.Lazy.mapStateT':

> local :: (Env -> Env) -> InterpM a -> InterpM a
> local f = mapStateT (R.local f)

We also wish to lift the operations of 'Control.Monad.Trans.Except.ExceptT'
through both 'Control.Monad.Trans.Reader.ReaderT' and
'Control.Monad.Trans.State.Lazy.StateT'.  For the operation
'Control.Monad.Trans.Except.throwE', we know @throwE e@ is a simple
action, so we can lift it through the two monad transformers to @InterpM@
with two 'lift's:

> throwE :: Err -> InterpM a
> throwE e = lift (lift (E.throwE e))

The 'Control.Monad.Trans.Except.catchE' operation has a more
complex type, so we need to use the special-purpose lifting function
@liftCatch@ provided by most monad transformers.  Here we use
the 'Control.Monad.Trans.Reader.ReaderT' version followed by the
'Control.Monad.Trans.State.Lazy.StateT' version:

> catchE :: InterpM a -> (Err -> InterpM a) -> InterpM a
> catchE = liftCatch (R.liftCatch E.catchE)

We could lift 'IO' actions to @InterpM@ using three 'lift's, but @InterpM@
is automatically an instance of 'Control.Monad.IO.Class.MonadIO',
so we can use 'Control.Monad.IO.Class.liftIO' instead:

> putStr :: String -> InterpM ()
> putStr s = liftIO (Prelude.putStr s)

-}
