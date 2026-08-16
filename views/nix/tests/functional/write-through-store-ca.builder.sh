#!/usr/bin/env bash

# The test greps for this line to tell a rebuild apart from a substitution.
echo "building-the-write-through-store-ca-fixture"

# shellcheck disable=SC2154
mkdir "$out"
echo "published by the build itself" > "$out"/marker
