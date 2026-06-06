.PHONY: help all setup deps build fmt fmt-check lint test ci quality

MIX ?= mix

help:
	@echo "Targets: setup, deps, fmt, fmt-check, lint, test, ci, quality"

setup:
	$(MIX) local.hex --force --if-missing
	$(MIX) local.rebar --force --if-missing
	$(MIX) setup

deps:
	$(MIX) deps.get

build:
	$(MIX) build

fmt:
	$(MIX) format

fmt-check:
	$(MIX) format --check-formatted

lint:
	$(MIX) lint

test:
	$(MIX) test

ci:
	$(MAKE) setup
	$(MAKE) build
	$(MAKE) fmt-check
	$(MAKE) lint
	$(MAKE) test

all: ci

# Non-required reporting gate (WS-8). Intentionally separate from ci/all so it
# never blocks PRs while the codebase is brought into compliance (WS-9).
# Depends on setup so a clean CI runner has Hex, rebar, and fetched deps
# before the tools run; without it `mix quality` aborts on a missing SCM.
quality: setup
	$(MIX) quality
	$(MIX) coveralls
