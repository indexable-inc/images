.. _hpc:


Hpc reference
=============

The hpc command has several sub-commands:

.. code-block:: none

    $ hpc
    Usage: hpc COMMAND ...

    Commands:
      help        Display help for hpc or a single command
    Reporting Coverage:
      report      Output textual report about program coverage
      markup      Markup Haskell source with program coverage
    Processing Coverage files:
      sum         Sum multiple .tix files in a single .tix file
      combine     Combine two .tix files in a single .tix file
      map         Map a function over a single .tix file
    Coverage Overlays:
      overlay     Generate a .tix file from an overlay file
      draft       Generate draft overlay that provides 100% coverage
    Others:
      show        Show .tix file in readable, verbose format
      version     Display version for hpc

In general, these options act on a ``.tix`` file after an instrumented
binary has generated it.

The hpc tool assumes you are in the top-level directory of the location
where you built your application, and the ``.tix`` file is in the same
top-level directory. You can use the flag ``--srcdir`` to use ``hpc``
for any other directory, and use ``--srcdir`` multiple times to analyse
programs compiled from difference locations, as is typical for packages.

We now explain in more details the major modes of hpc.

hpc report
^^^^^^^^^^

``hpc report`` gives a textual report of coverage. By default, all
modules and packages are considered in generating report, unless include
or exclude are used. The report is a summary unless the ``--per-module``
flag is used. The ``--xml-output`` option allows for tools to use hpc to
glean coverage.

.. code-block:: none

    $ hpc help report
    Usage: hpc report [OPTION] .. <TIX_FILE> [<MODULE> [<MODULE> ..]]

    Options:

        --per-module                  show module level detail
        --decl-list                   show unused decls
        --exclude=[PACKAGE:][MODULE]  exclude MODULE and/or PACKAGE
        --include=[PACKAGE:][MODULE]  include MODULE and/or PACKAGE
        --srcdir=DIR                  path to source directory of .hs files
                                      multi-use of srcdir possible
        --hpcdir=DIR                  append sub-directory that contains .mix files
                                      default .hpc [rarely used]
        --reset-hpcdirs               empty the list of hpcdir's
                                      [rarely used]
        --xml-output                  show output in XML

hpc markup
^^^^^^^^^^

``hpc markup`` marks up source files into colored html.

.. code-block:: none

    $ hpc help markup
    Usage: hpc markup [OPTION] .. <TIX_FILE> [<MODULE> [<MODULE> ..]]

    Options:

        --exclude=[PACKAGE:][MODULE]  exclude MODULE and/or PACKAGE
        --include=[PACKAGE:][MODULE]  include MODULE and/or PACKAGE
        --srcdir=DIR                  path to source directory of .hs files
                                      multi-use of srcdir possible
        --hpcdir=DIR                  append sub-directory that contains .mix files
                                      default .hpc [rarely used]
        --reset-hpcdirs               empty the list of hpcdir's
                                      [rarely used]
        --fun-entry-count             show top-level function entry counts
        --highlight-covered           highlight covered code, rather that code gaps
        --destdir=DIR                 path to write output to

hpc sum
^^^^^^^

``hpc sum`` adds together any number of ``.tix`` files into a single
``.tix`` file. ``hpc sum`` does not change the original ``.tix`` file;
it generates a new ``.tix`` file.

.. code-block:: none

    $ hpc help sum
    Usage: hpc sum [OPTION] .. <TIX_FILE> [<TIX_FILE> [<TIX_FILE> ..]]
    Sum multiple .tix files in a single .tix file

    Options:

        --exclude=[PACKAGE:][MODULE]  exclude MODULE and/or PACKAGE
        --include=[PACKAGE:][MODULE]  include MODULE and/or PACKAGE
        --output=FILE                 output FILE
        --union                       use the union of the module namespace (default is intersection)

hpc combine
^^^^^^^^^^^

``hpc combine`` is the swiss army knife of ``hpc``. It can be used to
take the difference between ``.tix`` files, to subtract one ``.tix``
file from another, or to add two ``.tix`` files. hpc combine does not
change the original ``.tix`` file; it generates a new ``.tix`` file.

.. code-block:: none

    $ hpc help combine
    Usage: hpc combine [OPTION] .. <TIX_FILE> <TIX_FILE>
    Combine two .tix files in a single .tix file

    Options:

        --exclude=[PACKAGE:][MODULE]  exclude MODULE and/or PACKAGE
        --include=[PACKAGE:][MODULE]  include MODULE and/or PACKAGE
        --output=FILE                 output FILE
        --function=FUNCTION           combine .tix files with join function, default = ADD
                                      FUNCTION = ADD | DIFF | SUB
        --union                       use the union of the module namespace (default is intersection)

hpc map
^^^^^^^

hpc map inverts or zeros a ``.tix`` file. hpc map does not change the
original ``.tix`` file; it generates a new ``.tix`` file.

.. code-block:: none

    $ hpc help map
    Usage: hpc map [OPTION] .. <TIX_FILE>
    Map a function over a single .tix file

    Options:

        --exclude=[PACKAGE:][MODULE]  exclude MODULE and/or PACKAGE
        --include=[PACKAGE:][MODULE]  include MODULE and/or PACKAGE
        --output=FILE                 output FILE
        --function=FUNCTION           apply function to .tix files, default = ID
                                      FUNCTION = ID | INV | ZERO
        --union                       use the union of the module namespace (default is intersection)

hpc overlay and hpc draft
^^^^^^^^^^^^^^^^^^^^^^^^^

Overlays are an experimental feature of HPC, a textual description of
coverage. hpc draft is used to generate a draft overlay from a .tix
file, and hpc overlay generates a .tix files from an overlay.

.. code-block:: none

    % hpc help overlay
    Usage: hpc overlay [OPTION] .. <OVERLAY_FILE> [<OVERLAY_FILE> [...]]

    Options:

        --srcdir=DIR   path to source directory of .hs files
                       multi-use of srcdir possible
        --hpcdir=DIR                  append sub-directory that contains .mix files
                                      default .hpc [rarely used]
        --reset-hpcdirs               empty the list of hpcdir's
                                      [rarely used]
        --output=FILE  output FILE
    % hpc help draft
    Usage: hpc draft [OPTION] .. <TIX_FILE>

    Options:

        --exclude=[PACKAGE:][MODULE]  exclude MODULE and/or PACKAGE
        --include=[PACKAGE:][MODULE]  include MODULE and/or PACKAGE
        --srcdir=DIR                  path to source directory of .hs files
                                      multi-use of srcdir possible
        --hpcdir=DIR                  append sub-directory that contains .mix files
                                      default .hpc [rarely used]
        --reset-hpcdirs               empty the list of hpcdir's
                                      [rarely used]
        --output=FILE                 output FILE
