# shellcheck shell=bash
# Classifying a failure by error class, shared by every differential gate.
#
# Sourced rather than copied: lang-diff.sh and drv-parity.sh both need it, and
# the second copy is how the two drift into disagreeing about what "the same
# failure" means. Both arms failing is not by itself agreement -- cppnix
# rejecting a name and the Rust VM running out of memory are both non-zero --
# so the class is what makes "both failed" mean something.

# The last "error: …" line, unindented, with ANSI stripped. This is the error
# itself; everything above it is cppnix trace decoration. Colour is stripped
# because nix writes it whenever the stream looks like a terminal, and a
# pattern matched against escaped text silently matches nothing.
last_error() {
  # LC_ALL=C so sed treats the stream as bytes: an error message can carry
  # invalid UTF-8 (eval-fail-toJSON-non-utf-8 does), and BSD sed aborts on it
  # under a UTF-8 locale rather than passing it through.
  LC_ALL=C sed -e 's/\x1b\[[0-9;]*m//g' "$1" \
    | LC_ALL=C grep -a -E '^[[:space:]]*error: ' | tail -1 | LC_ALL=C sed -e 's/^[[:space:]]*//'
}

# Every grep here runs `LC_ALL=C grep -a`, and both halves are load-bearing.
# An error message can carry bytes that are not text: eval-fail-string-nul-*
# puts a literal NUL in the message and eval-fail-toJSON-non-utf-8 puts
# invalid UTF-8 there. Without -a, grep calls the stream binary and prints no
# matching line (while still reporting a count, so -c looks fine); without
# LC_ALL=C, sed aborts on the invalid sequence. Either way every pattern comes
# back false and the pair lands in `unknown` for a reason that has nothing to
# do with the error -- which is how these two pairs stayed mismatched through
# three rounds of fixing the messages themselves.
error_class() { # stderr file -> class token
  local f=$1
  if LC_ALL=C grep -aq 'rust-eval unimplemented' "$f"; then echo unimplemented
  elif LC_ALL=C grep -aq 'rust-eval parse error' "$f"; then echo parse
  elif LC_ALL=C grep -aq 'syntax error' "$f"; then echo parse
  # cppnix raises these from the parser too, without the word "syntax".
  elif LC_ALL=C grep -aqE 'dynamic attributes not allowed in (let|inherit)|attribute .* already defined' "$f"; then echo parse
  # cppnix throws this one from the parser, so it is a parse-class failure
  # even though the message never says "syntax".
  elif LC_ALL=C grep -aq 'path has a trailing slash' "$f"; then echo parse
  elif LC_ALL=C grep -aq 'undefined variable' "$f"; then echo undefined-variable
  elif LC_ALL=C grep -aq 'infinite recursion encountered' "$f"; then echo infinite-recursion
  elif LC_ALL=C grep -aq 'stack overflow' "$f"; then echo stack-overflow
  elif LC_ALL=C grep -aqE "attribute '[^']*' missing" "$f"; then echo missing-attr
  elif LC_ALL=C grep -aqE 'assertion( .*)? failed' "$f"; then echo assert
  elif LC_ALL=C grep -aqE "is not equal to|differs from attribute set|is contained in '.*', but not in|is missing in '.*', but is contained in|immediate comparisons of identical functions compare as unequal" "$f"; then echo assert
  # String-context validation, which both evaluators refuse and word almost
  # identically -- cppnix prints the offending key through its Nix-string
  # printer, so the path arrives wrapped in a second pair of quotes. Without
  # a token of their own these landed in `unknown`, where the comparison is
  # byte equality of the terminal line, and the quoting alone decided five
  # drv-parity pairs. The class is what the gate is asking about; the quoting
  # is the `error-text` tier.
  elif LC_ALL=C grep -aqE "context key .* is not a store path|tried to add (all-outputs|derivation output) context of .*, which is not a derivation" "$f"; then echo context
  elif LC_ALL=C grep -aq 'evaluation aborted' "$f"; then echo abort
  elif LC_ALL=C grep -aq "'throw' builtin" "$f"; then echo throw
  elif LC_ALL=C grep -aqE 'expected a [a-z ]+ but found|cannot coerce|is a [a-z]+ while a [a-z]+ was expected|cannot compare|has no attribute|called with unexpected argument|called without required argument|cannot convert' "$f"; then echo type
  else echo unknown; fi
}
