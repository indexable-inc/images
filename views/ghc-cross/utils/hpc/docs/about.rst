.. _about:

Observing Code Coverage with HPC
================================

Code coverage tools allow a programmer to determine what parts of their
code have been actually executed, and which parts have never actually
been invoked. GHC has an option for generating instrumented code that
records code coverage as part of the Haskell Program Coverage (HPC)
toolkit, which is included with GHC. HPC tools can be used to render the
generated code coverage information into human understandable format.

Correctly instrumented code provides coverage information of two kinds:
source coverage and boolean-control coverage. Source coverage is the
extent to which every part of the program was used, measured at three
different levels: declarations (both top-level and local), alternatives
(among several equations or case branches) and expressions (at every
level). Boolean coverage is the extent to which each of the values True
and False is obtained in every syntactic boolean context (ie. guard,
condition, qualifier).

HPC displays both kinds of information in two primary ways: textual
reports with summary statistics (``hpc report``) and sources with color
mark-up (``hpc markup``). For boolean coverage, there are four possible
outcomes for each guard, condition or qualifier: both True and False
values occur; only True; only False; never evaluated. In hpc-markup
output, highlighting with a yellow background indicates a part of the
program that was never evaluated; a green background indicates an
always-True expression and a red background indicates an always-False
one.

About hpc-bin
=============

The HPC binary is a tool for generating coverage reports in various output formats for Haskell projects.
The latest source code lives in the `git repository <https://gitlab.haskell.org/hpc/hpc-bin>`__ on ``GitLab``.

Reporting bugs in Hpc
=====================

Please report bugs on the `Hpc-bin issue tracker <https://gitlab.haskell.org/hpc/hpc-bin/-/issues>`__.
