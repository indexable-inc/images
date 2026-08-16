# cppnix warns that `structuredAttrs` disables `allowedReferences`, and so
# does this evaluator. The warning is an output the memo table has to carry:
# a result served from cache that stayed quiet would tell its reader less than
# the run that filled the cache did, which is ENG-12540 in the one form the
# value comparison cannot see.
#
# Refused rather than answered when no store directory is configured, which is
# why the gate's `default` configuration does not witness this one and
# `store-elsewhere` does.
(builtins.derivationStrict {
  name = "cachesem-warns";
  system = "x86_64-linux";
  builder = "/bin/sh";
  __structuredAttrs = true;
  allowedReferences = [ ];
}).out
