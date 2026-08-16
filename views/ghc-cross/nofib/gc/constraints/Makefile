TOP = ../..
include $(TOP)/mk/boilerplate.mk

FAST_OPTS = 6
NORM_OPTS = 7
SLOW_OPTS = 8

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H330m -RTS
endif
ifeq "$(HEAP)" "OLD"
PROG_ARGS += +RTS -H10m -RTS
endif

include $(TOP)/mk/target.mk
