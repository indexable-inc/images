TOP = ../..

include $(TOP)/mk/boilerplate.mk

all boot :: input

input : words
	cat words words words words words words words words words words >$@

SRC_DEPS = containers
NORM_OPTS = words input

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H32m -RTS
endif
ifeq "$(HEAP)" "OLD"
PROG_ARGS += +RTS -H10m -RTS
endif

include $(TOP)/mk/target.mk
