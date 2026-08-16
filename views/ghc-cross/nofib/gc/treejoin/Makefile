TOP = ../..
include $(TOP)/mk/boilerplate.mk

# Arguments to test program
PROG_ARGS = 27000.1 27000.2

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H32m -RTS
endif
ifeq "$(HEAP)" "OLD"
PROG_ARGS += +RTS -H24m -RTS
endif

include $(TOP)/mk/target.mk
