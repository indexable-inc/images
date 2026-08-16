TOP = ../..

include $(TOP)/mk/boilerplate.mk

NORM_OPTS = 14

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H8m -RTS
endif
ifeq "$(HEAP)" "OLD"
PROG_ARGS += +RTS -H10m -RTS
endif

include $(TOP)/mk/target.mk
