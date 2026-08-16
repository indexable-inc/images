TOP = ../..
include $(TOP)/mk/boilerplate.mk

# Override default SRCS; the default is all source files, but
# we don't want to include revcomp-c.c
SRCS = Main.hs

CLEAN_FILES += fasta-c revcomp-c reverse-complement *stdout *stdin

# These values are only used in this file. They are ignored by the
# executable itself.
FAST_OPTS =  5000000
NORM_OPTS = 50000000
# Normally, we'd want to have SLOW_OPTS = $((5*NORM_OPTS)) to run this for 5s
# in 2018. But that means
#   - slow generation of huge input and output files
#   - having to diff the generated output files
#   - the input parser in fasta-c.c overflows, so we're generating an incorrect
#     reference output
# So we don't do it.
SLOW_OPTS = 50000000
# This is the official shootout setting ($((NORM_OPTS/2)), i.e. running 0.5s in
# 2018):
# SLOW_OPTS = 25000000

# The benchmark game also uses -fllvm, which we can't since it might
# not be available on the developer's machine.

# Since we only decide here what the INPUT_FILE is, it's required to first run
# `make boot` and only than `make` (otherwise `make` doesn't "see" the file and
# doesn't call `runstdtest` correctly)
ifeq "$(mode)" "slow"
 INPUT_FILE = reverse-complement.slowstdin
 OUTPUT_FILE = reverse-complement.slowstdout
else
 ifeq "$(mode)" "fast"
  INPUT_FILE = reverse-complement.faststdin
  OUTPUT_FILE = reverse-complement.faststdout
 else
  INPUT_FILE = reverse-complement.stdin
  OUTPUT_FILE = reverse-complement.stdout
 endif
endif

boot :: $(INPUT_FILE) $(OUTPUT_FILE)

# Input/Output files generated with boot.sh script
# This is a "pattern rule", the '%' is required. Just a trick to tell make
# that this command generates multiple files.
reverse-complement%faststdin reverse-complement%faststdout: boot.sh ../fasta/fasta-c.c
	./boot.sh . . ${HC} $(mode)

reverse-complement%stdin reverse-complementi%stdout: boot.sh ../fasta/fasta-c.c
	./boot.sh . . ${HC} $(mode)

reverse-complement%slowstdin reverse-complement%slowstdout: boot.sh ../fasta/fasta-c.c
	./boot.sh . . ${HC} $(mode)

include $(TOP)/mk/target.mk
