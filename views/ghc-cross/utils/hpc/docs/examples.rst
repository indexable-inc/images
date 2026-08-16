.. _examples:

A small example: Reciprocation
==============================

For an example we have a program, called :file:`Recip.hs`, which computes
exact decimal representations of reciprocals, with recurring parts
indicated in brackets. ::

    reciprocal :: Int -> (String, Int)
    reciprocal n | n > 1 = ('0' : '.' : digits, recur)
                 | otherwise = error
                  "attempting to compute reciprocal of number <= 1"
      where
      (digits, recur) = divide n 1 []
    divide :: Int -> Int -> [Int] -> (String, Int)
    divide n c cs | c `elem` cs = ([], position c cs)
                  | r == 0      = (show q, 0)
                  | r /= 0      = (show q ++ digits, recur)
      where
      (q, r) = (c*10) `quotRem` n
      (digits, recur) = divide n r (c:cs)

    position :: Int -> [Int] -> Int
    position n (x:xs) | n==x      = 1
                      | otherwise = 1 + position n xs

    showRecip :: Int -> String
    showRecip n =
      "1/" ++ show n ++ " = " ++
      if r==0 then d else take p d ++ "(" ++ drop p d ++ ")"
      where
      p = length d - r
      (d, r) = reciprocal n

    main = do
      number <- readLn
      putStrLn (showRecip number)
      main

HPC instrumentation is enabled with the `-fhpc` flag:

.. code-block:: sh

    $ ghc -fhpc Recip.hs

GHC creates a subdirectory ``.hpc`` in the current directory, and puts
HPC index (``.mix``) files in there, one for each module compiled. You
don't need to worry about these files: they contain information needed
by the ``hpc`` tool to generate the coverage data for compiled modules
after the program is run.

.. code-block:: sh

    $ ./Recip
    1/3
    = 0.(3)

Running the program generates a file with the ``.tix`` suffix, in this
case :file:`Recip.tix`, which contains the coverage data for this run of the
program. The program may be run multiple times (e.g. with different test
data), and the coverage data from the separate runs is accumulated in
the ``.tix`` file. To reset the coverage data and start again, just
remove the ``.tix`` file.

Having run the program, we can generate a textual summary of coverage:

.. code-block:: none

    $ hpc report Recip
     80% expressions used (81/101)
     12% boolean coverage (1/8)
          14% guards (1/7), 3 always True,
                            1 always False,
                            2 unevaluated
           0% 'if' conditions (0/1), 1 always False
         100% qualifiers (0/0)
     55% alternatives used (5/9)
    100% local declarations used (9/9)
    100% top-level declarations used (5/5)

We can also generate a marked-up version of the source.

.. code-block:: none

    $ hpc markup Recip
    writing Recip.hs.html

This generates one file per Haskell module, and 4 index files,
:file:`hpc_index.html`, :file:`hpc_index_alt.html`, :file:`hpc_index_exp.html`,
:file:`hpc_index_fun.html`.
