TOP = ../..
include $(TOP)/mk/boilerplate.mk

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H16m -RTS
endif
ifeq "$(HEAP)" "OLD"
PROG_ARGS += +RTS -H10m -RTS
endif

include $(TOP)/mk/target.mk

