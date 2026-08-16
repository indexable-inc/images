TOP = ../..
include $(TOP)/mk/boilerplate.mk

# Override default SRCS; the default is all source files, but
# we don't want to include fasta-c.c
SRCS = Main.hs

CLEAN_FILES += fasta-c fasta.faststdout fasta.stdout fasta.slowstdout

# If you modify these also modify boot.sh
FAST_OPTS = 750000
NORM_OPTS = 7500000
SLOW_OPTS = 25000000  # official shootout setting

# The benchmark game also uses -fllvm, which we can't since it might
# not be available on the developer's machine.
SRC_DEPS = bytestring

#------------------------------------------------------------------
# Create output to validate against

boot :: fasta.stdout

fasta.stdout: boot.sh
	./boot.sh . . ${HC} $(mode)

include $(TOP)/mk/target.mk
