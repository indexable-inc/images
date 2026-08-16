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

FAST_OPTS=60000
NORM_OPTS=600000
SLOW_OPTS=3000000

case $mode in
"norm"*)
  MODE_ARGS=$NORM_OPTS
  INPUT_FILE=k-nucleotide.stdin
  ;;
"slow"*)
  MODE_ARGS=$SLOW_OPTS
  INPUT_FILE=k-nucleotide.slowstdin
  ;;
*)
  MODE_ARGS=$FAST_OPTS
  INPUT_FILE=k-nucleotide.faststdin
  ;;
esac

set -x
${HC} -O2 ${src}/../fasta/fasta-c.c -c -o ${output}/fasta-c.o
${HC} -O2 ${output}/fasta-c.o -o ${output}/fasta-c$EXE -no-hs-main
${output}/fasta-c${EXE} $MODE_ARGS | tr -d '\r' > ${output}/${INPUT_FILE}

# k-nucleotide.faststdin : fasta-c
# 	./fasta-c $(FAST_OPTS) | tr -d '\r' > $@

# k-nucleotide.stdin : fasta-c
# 	./fasta-c $(NORM_OPTS) | tr -d '\r' > $@

# k-nucleotide.slowstdin : fasta-c
# 	./fasta-c $(SLOW_OPTS) | tr -d '\r' > $@
