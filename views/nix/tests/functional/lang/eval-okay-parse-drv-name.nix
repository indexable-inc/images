# builtins.parseDrvName over the edge cases of cppnix's DrvName constructor
# (src/libstore/names.cc:23): the name is everything up to the first dash NOT
# followed by a letter, the version is the rest without that dash.
#
# eval-okay-versions.nix already calls parseDrvName, but it reduces every case
# to one `true`, so a differ comparing the two evaluators sees a single boolean
# and cannot tell which case moved. This file returns the parsed sets.
let
  parse = name: {
    inherit name;
    parsed = builtins.parseDrvName name;
  };
in
map parse [
  # cppnix's own unit test (src/libexpr-tests/primops.cc:858).
  "nix-0.12pre12876"
  "a-b-c-1234pre5+git"
  # A trailing dash is not a separator: the loop's condition includes
  # `i + 1 < s.size()`.
  "hello-"
  "-"
  ""
  # The test is `!isalpha`, not "is a digit", so a doubled dash separates at
  # the first of the two and the second stays in the version.
  "--"
  "a--1"
  # A leading dash leaves an empty name.
  "-1"
  # A letter after the dash never separates, either case.
  "foo-bar"
  "foo-Bar"
  # Anything else does.
  "foo-9"
  "foo-_"
  "foo-."
  # First match wins; later dashes belong to the version.
  "a-b-1-c"
  # No dash at all.
  "1234"
  "1.2.3"
  # `isalpha` is ASCII in the C locale, so a dash before a multi-byte
  # character separates; the dash before `a` in the last one does not.
  "foo-é"
  "café-1"
  "é-a"
]
