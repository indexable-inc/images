#!/bin/bash

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

FAST_OPTS=3500
NORM_OPTS=35000
SLOW_OPTS=180000  # official shootout setting

case $mode in
"fast"*)
  MODE_ARGS=$FAST_OPTS
  ;;
"slow"*)
  MODE_ARGS=$SLOW_OPTS
  ;;
*)
  MODE_ARGS=$NORM_OPTS
  ;;
esac

set -e
set -x

# We could use perl/python for this. But it's not much faster than ghci and adds additional dependencies.
# We could use bash directly. But it's incredibly slow on windows.
# So instead we just use ghci which is fast enough for this purpose.
$HC --interactive -package-env - -e "readFile \"${src//\\/\\\\}/banner.stdout.template\" >>= \\s -> writeFile \"${output//\\/\\\\}/banner.stdout\" (concat . replicate ${MODE_ARGS} $ s)"
$HC --interactive -package-env - -e "readFile \"${src//\\/\\\\}/banner.stdin.template\" >>= \\s -> writeFile \"${output//\\/\\\\}/banner.stdin\"  (concat . replicate ${MODE_ARGS} $ s)"
