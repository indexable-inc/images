#!/usr/bin/env bash

# A symlink into the source tree, the shape `separateDebugInfo` writes under
# `.build-id`. The reference scanner reads symlink targets, so this is what makes
# the evaluator-added source a hard reference of the output rather than merely an
# input of the derivation.
# shellcheck disable=SC2154
mkdir "$out"
ln -s "$evaluatorAddedSource" "$out"/source
