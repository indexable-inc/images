TOP = ../..
include $(TOP)/mk/boilerplate.mk

FAST_OPTS = 1000000
NORM_OPTS = 5000000
SLOW_OPTS = 100000000

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H430m -RTS
endif

include $(TOP)/mk/target.mk
