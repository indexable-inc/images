TOP = ../..
include $(TOP)/mk/boilerplate.mk

SRC_DEPS = old-time array

# kLongLivedTreeDepth	  = 17 :: Int
# kArraySize	 	     = 500000 :: Int
# kMinTreeDepth 	     = 4 :: Int
# kMaxTreeDepth 	     = 17 :: Int

FAST_OPTS = 17 400000 4 17
NORM_OPTS = 18 500000 4 19
SLOW_OPTS = 19 500000 5 22

ifeq "$(HEAP)" "LARGE"
PROG_ARGS += +RTS -H180m -RTS
endif

include $(TOP)/mk/target.mk

