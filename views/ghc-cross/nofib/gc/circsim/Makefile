TOP = ../..

include $(TOP)/mk/boilerplate.mk

FAST_OPTS = 8   4
NORM_OPTS = 8  40
SLOW_OPTS = 8 200

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H256m -RTS
endif
ifeq "$(HEAP)" "OLD"
PROG_ARGS += +RTS -H30m -RTS
endif

include $(TOP)/mk/target.mk
