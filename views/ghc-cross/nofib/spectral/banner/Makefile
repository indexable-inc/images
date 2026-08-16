TOP = ../..
include $(TOP)/mk/boilerplate.mk
-include opts.mk
include $(TOP)/mk/target.mk

CLEAN_FILES += banner.stdin banner.stdout

# Only works because we don't use parallel make
banner.stdin banner.stdout: boot.sh banner.stdin.template banner.stdout.template
	bash boot.sh . . $(HC) $(mode)

boot :: banner.stdout banner.stdin
