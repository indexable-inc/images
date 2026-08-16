TOP = ../..
include $(TOP)/mk/boilerplate.mk
-include opts.mk
include $(TOP)/mk/target.mk

# When you change these, make sure to update the boot.sh
# script as well
FAST_OPTS =   2000
NORM_OPTS =  20000
SLOW_OPTS = 100000

CLEAN_FILES += awards.stdout awards.slowstdout

awards.stdout : awards.out_template
	./boot.sh . . $(HC) $(mode)

boot :: awards.stdout
