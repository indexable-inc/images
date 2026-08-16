TOP = ../..
include $(TOP)/mk/boilerplate.mk
include $(TOP)/mk/target.mk

FAST_OPTS = 1500
NORM_OPTS = 5000
SLOW_OPTS = 12000  # official shootout setting is 5500

# The benchmark game also uses -fllvm, which we can't since it might
# not be available on the developer's machine.
