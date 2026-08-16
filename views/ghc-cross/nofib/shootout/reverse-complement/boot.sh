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

# These values are only used in this file. They are ignored by the
# executable itself.
FAST_OPTS=5000000
NORM_OPTS=50000000
# Normally, we'd want to have SLOW_OPTS = $((5*NORM_OPTS)) to run this for 5s
# in 2018. But that means
#   - slow generation of huge input and output files
#   - having to diff the generated output files
#   - the input parser in fasta-c.c overflows, so we're generating an incorrect
#     reference output
# So we don't do it.
SLOW_OPTS=50000000
# This is the official shootout setting ($((NORM_OPTS/2)), i.e. running 0.5s in
# 2018):
# SLOW_OPTS = 25000000

# The benchmark game also uses -fllvm, which we can't since it might
# not be available on the developer's machine.

case $mode in
"norm"*)
  MODE_ARGS=$NORM_OPTS
  INPUT_FILE=reverse-complement.stdin
  OUTPUT_FILE=reverse-complement.stdout
  ;;
"slow"*)
  MODE_ARGS=$SLOW_OPTS
  INPUT_FILE=reverse-complement.slowstdin
  OUTPUT_FILE=reverse-complement.slowstdout
  ;;
*)
  MODE_ARGS=$FAST_OPTS
  INPUT_FILE=reverse-complement.faststdin
  OUTPUT_FILE=reverse-complement.faststdout
  ;;
esac

#
# Create Input
#
set -x
${HC} -O2 ${src}/../fasta/fasta-c.c -c -o ${output}/fasta-c.o
${HC} -O2 ${output}/fasta-c.o -o ${output}/fasta-c$EXE -no-hs-main
${output}/fasta-c${EXE} $MODE_ARGS | tr -d '\r' > ${output}/${INPUT_FILE}

#
# Create Output for validation
#
${HC} -O2 ${src}/revcomp-c.c -c -o ${output}/revcomp-c.o
${HC} -O2 ${output}/revcomp-c.o -o ${output}/revcomp-c$EXE -no-hs-main
${output}/revcomp-c$EXE < ${output}/${INPUT_FILE} | tr -d '\r' > ${output}/${OUTPUT_FILE}

