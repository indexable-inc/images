TOP = ../..
include $(TOP)/mk/boilerplate.mk

NORM_OPTS = 300000

SRC_DEPS = array
PROG_ARGS += +RTS -K64m -RTS

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H128m -RTS
endif
ifeq "$(HEAP)" "OLD"
PROG_ARGS += +RTS -H10m -RTS
endif

include $(TOP)/mk/target.mk
