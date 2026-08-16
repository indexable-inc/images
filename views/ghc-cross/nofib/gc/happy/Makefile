TOP = ../..

include $(TOP)/mk/boilerplate.mk

NORM_OPTS = TestInput.y
SRC_HC_OPTS += -cpp
SRC_DEPS = containers
EXCLUDED_SRCS += TestInput.hs

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H128m -RTS
endif
ifeq "$(HEAP)" "OLD"
PROG_ARGS += +RTS -H10m -RTS
endif

include $(TOP)/mk/target.mk
