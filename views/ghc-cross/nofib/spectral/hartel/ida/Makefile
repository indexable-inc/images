TOP = ../../..
include $(TOP)/mk/boilerplate.mk

# The first parameter varies the number of iterations,
# the second parameter is actual input. My measurements
# in #15999 show that we probably want to have a high
# first parameter for stable results.
# SG
FAST_OPTS = 1500 2
NORM_OPTS =  600 3
SLOW_OPTS = 3000 3

include $(TOP)/mk/target.mk

