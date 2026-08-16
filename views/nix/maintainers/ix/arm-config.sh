# shellcheck shell=bash
# One owner of the nix configuration every differential gate runs under.
#
# Sourced rather than copied, for the reason `error-class.sh` and
# `compare-arms.sh` are: the second copy is how two gates come to disagree
# about what they are measuring. Five of them had their own copy of a
# three-line lint list before this existed (ENG-12996).
#
# ## The problem is the ambient configuration, not the lint list
#
# `NIX_CONFIG` is applied ON TOP OF whatever conf files are in scope, so a gate
# that names three settings inherits everything else the machine happens to
# say. Two ways that has gone wrong, both measured on one developer Mac:
#
#   - `lint-url-literals = fatal` in ~/.config/nix/nix.conf makes the rust arm
#     refuse EVERY evaluation by name -- it has no parser lint to honour a
#     fatal setting with. A gate without a capability probe then scores every
#     row `unimplemented`, keeps `mismatch` at 0 and exits 0 having measured
#     nothing. `drv-parity.sh` exits 2 at its probe on that machine today.
#   - `lint-absolute-path-literals = warn` produces six FALSE mismatches in
#     `drv-parity.sh`, because the cpp arm prints a lint warning and the rust
#     arm structurally cannot. They read as regressions and are not.
#
# Adding a sixth copy of the lint list fixes neither. What fixes both is
# owning the configuration: drop the user conf file and name what the gates
# need.
#
# ## What this does and does not neutralise
#
# `NIX_USER_CONF_FILES=/dev/null` drops ~/.config/nix/nix.conf and nothing
# else. `/etc/nix/nix.conf` still applies, deliberately: that is where this
# machine's `experimental-features` comes from (flakes, nix-command,
# ca-derivations and the rest), and a gate that lost those would be measuring
# a nix nobody runs. Measured here: with the user conf dropped,
# `lint-url-literals` goes `fatal` -> `ignore` and `experimental-features` is
# unchanged.
#
# So this is not hermetic and does not claim to be. It removes the layer a
# developer edits, which is the layer that has actually broken gates.
#
# ## Why the lints are `ignore` and not `warn`
#
# The convention this replaces used `warn`, so that a corpus case tripping a
# lint would still say so. That reasoning only holds if BOTH arms can say so,
# and they cannot: the rust backend has no parser lint at all. At `warn` the
# cpp arm prints a warning the rust arm is structurally incapable of printing,
# which is a guaranteed difference in every row whose expression contains an
# absolute path -- the six false mismatches above. At `ignore` both arms are
# silent and the comparison is about the evaluator again.
#
# The lint coverage that `warn` was reaching for is not lost; it is just not
# this gate's job. It belongs in a gate that runs ONE arm and asserts the lint
# fires, which can be written without making every differential row noisy.

# The settings every arm gets, before the arm's own. Newline-separated, in the
# `NIX_CONFIG` format.
#
# `experimental-features` is not named here: it comes from /etc/nix and each
# gate adds the ones it needs with `extra-experimental-features`, which is
# additive and so cannot take away something the machine legitimately has.
# Refuses if the environment was never pinned, and that check rides HERE
# rather than in a separate call a gate has to remember.
#
# Two earlier versions of this file had the check as its own function, and
# both times it ended up inert without anyone noticing: once called before the
# file was sourced (`command not found`, non-fatal under `set -u`), once
# placed in a region only one of the gate's two arms reaches. Both were caught
# only by deliberately breaking the pin and finding the gate still passed.
#
# A check on the function every arm must call to get its configuration cannot
# be skipped by a caller that got its configuration, which is the difference
# between a guard and a convention.
arm_base_config() {
  if [ "${NIX_USER_CONF_FILES:-}" != /dev/null ]; then
    echo "arm-config: NIX_USER_CONF_FILES is '${NIX_USER_CONF_FILES:-<unset>}', not /dev/null." >&2
    echo "  This gate asked for the shared configuration without calling" >&2
    echo "  arm_pin_environment first, so every arm would still inherit" >&2
    echo "  ~/.config/nix/nix.conf -- where a 'lint-url-literals = fatal' makes the" >&2
    echo "  rust arm refuse every row. Refusing to hand out a configuration that" >&2
    echo "  would not be in force." >&2
    exit 2
  fi
  printf '%s\n' \
    'lint-url-literals = ignore' \
    'lint-short-path-literals = ignore' \
    'lint-absolute-path-literals = ignore'
}

# Run a command with the gate's configuration in force.
#
#   arm_run "$cfg" mybinary eval --raw ...
#
# `$1` is the arm's own NIX_CONFIG text, appended after the base so an arm can
# still override a base setting deliberately -- `NIX_CONFIG` takes the last
# value for a repeated key.
arm_run() { # NIX_CONFIG_TEXT COMMAND...
  local cfg=$1
  shift
  NIX_USER_CONF_FILES=/dev/null NIX_CONFIG="$(arm_base_config)
$cfg" "$@"
}

# The environment assignments, for a caller that builds its own command line
# rather than going through `arm_run` -- `env`-style, one per line.
#
# Exists because some gates need the config in a subshell, a `timeout`, or a
# background job where wrapping the command is awkward. Same two variables
# either way, so there is still one place that decides them.
arm_export_config() { # NIX_CONFIG_TEXT
  local base
  base=$(arm_base_config)
  export NIX_USER_CONF_FILES=/dev/null
  export NIX_CONFIG="$base
$1"
}

# Drop the developer's conf file for this script and everything it runs.
#
# Called once, near the top of a gate, rather than folded into `arm_run`: some
# gates build their own command lines, background jobs and `timeout` wrappers,
# and an exported variable reaches all of them where a wrapper reaches only
# what goes through it. `arm_require_clean_config` below refuses a gate that
# forgot to call this, so the coverage is checked rather than assumed.
arm_pin_environment() {
  export NIX_USER_CONF_FILES=/dev/null
}

# Refuse a run whose ambient configuration is still leaking in.
#
# A guard rather than a comment, because the failure it catches is silent: if
# `NIX_USER_CONF_FILES` stops working, or a gate builds `NIX_CONFIG` without
# going through here, every arm quietly inherits the developer's settings
# again and the only symptom is a score that moved. `$1` is a binary that
# accepts `config show`.
arm_require_clean_config() { # NIX_BINARY
  local nixbin=$1 got
  # The environment half is checked by `arm_base_config` itself, which the
  # line below calls, so this function only adds what needs a binary: that the
  # setting is actually in force in the process that will evaluate.
  if ! got=$(NIX_CONFIG="$(arm_base_config)" "$nixbin" config show lint-url-literals 2>&1); then
    # Told apart from a leak deliberately. The first version of this guard
    # folded the two together and reported "the ambient configuration is still
    # in scope" when it had in fact been handed `nix-instantiate`, which has no
    # `config show` -- a wrong diagnosis pointing at the wrong file.
    echo "arm-config: '$nixbin config show' failed, so this guard could not ask" >&2
    echo "  anything. It needs a binary with the modern CLI ('nix'), not" >&2
    echo "  nix-instantiate. Got: $got" >&2
    exit 2
  fi
  if [ "$got" != ignore ]; then
    echo "arm-config: lint-url-literals reads '$got' where this file sets 'ignore'." >&2
    echo "  Either the ambient configuration is still in scope -- check that" >&2
    echo "  NIX_USER_CONF_FILES reached the binary -- or /etc/nix/nix.conf sets it," >&2
    echo "  which this file cannot drop and the operator has to. Refusing to score," >&2
    echo "  because the arms would be measuring the machine and not the evaluator." >&2
    exit 2
  fi
}
