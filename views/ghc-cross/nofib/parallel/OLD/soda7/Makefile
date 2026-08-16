TOP = ../..
include $(TOP)/mk/boilerplate.mk

# Override default SRCS; the default is all source files
SRCS=Main.hs
PROG_ARGS += 20
SRC_HC_OPTS += -cpp -package concurrent

include $(TOP)/mk/target.mk

