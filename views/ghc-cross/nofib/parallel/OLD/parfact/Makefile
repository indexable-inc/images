TOP = ../..
include $(TOP)/mk/boilerplate.mk
-include opts.mk

# Override default SRCS; the default is all source files
SRCS=parfact.hs
PROG_ARGS += 8000000 1000
SRC_HC_OPTS += -package concurrent

include $(TOP)/mk/target.mk

