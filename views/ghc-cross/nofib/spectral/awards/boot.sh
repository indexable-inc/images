#!/usr/bin/env bash

# Arguments:
# $1 - src folder
# $2 - output folder
# $3 - path to ghc
# $4 - mode

set -e
src=${1:-.}
output=${2:-.}
HC=${3:-ghc}
mode=${4:-norm}

EXE=""
if [[ $(uname -s | grep -c 'MSYS\|MINGW') -eq 1 ]]
then
  EXE=".exe"
fi

FAST_OPTS=2000
NORM_OPTS=20000
SLOW_OPTS=100000

case $mode in
"fast"*)
  MODE_ARGS=$FAST_OPTS
  REPS=1
  ;;
"slow"*)
  MODE_ARGS=$SLOW_OPTS
  REPS=50
  ;;
*)
  MODE_ARGS=$NORM_OPTS
  REPS=10
  ;;
esac

set -e
set -x

# We could use perl/python for this. But it's not much faster than ghci and adds additional dependencies.
# We could use bash directly. But it's incredibly slow on windows.
# So instead we just use ghci which is fast enough for this purpose.
# ${src//\\/\\\\} => Replace all patterns of a single backslash with double backslashes
$HC --interactive -package-env - -e  "readFile \"${src//\\/\\\\}/awards.out_template\" >>= \\s -> writeFile \"${output//\\/\\\\}/awards.stdout\" (concat . replicate ${REPS} $ s)"
