# shellcheck shell=bash
source common.sh
source ../common/init.sh

requireEnvironment
setupConfig
enableFeatures "ca-derivations"
execUnshare ./lower-only-realisation-inner.sh
