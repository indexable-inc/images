# Per-step wall-clock timings for Home Manager activation.
#
# Home Manager prints one untimed "Activating <step>" line per activation
# entry; every entry runs in the same bash process and announces itself
# through `_iNote "Activating %s" <name>` in the generated script. The entry
# below sorts first and interposes on that exact call: each new header first
# reports how long the previous step took, and an EXIT trap prints a
# slowest-steps summary plus the total (indexable-inc/index#3673).
#
# The summary rides the EXIT trap rather than a dag entry: the dag has no
# global sink, so entries without ordering constraints can legally sort
# after any fixed anchor and would escape a summary entry (observed with
# mutableFiles and friends). The generated script already arms an EXIT trap
# (the new-generation gcroot cleanup), so the trap body chains to it.
#
# All math is bash integer arithmetic on $EPOCHREALTIME: the activation
# script's PATH carries only the Home Manager tool closure (coreutils, sed,
# grep, jq), so awk/bc are unavailable and, under `set -e`, fatal.
{lib, ...}: {
  home.activation.activationTimerStart = lib.hm.dag.entryBefore ["checkFilesChanged" "checkLinkTargets"] ''
    # Milliseconds since the epoch; EPOCHREALTIME is "sec.micros" with a
    # locale-dependent radix character, and the microsecond field's leading
    # zeros would otherwise read as octal (hence 10#).
    _ixActNowMs() {
      local t=$EPOCHREALTIME
      printf '%s' "$(( ''${t%[.,]*} * 1000 + 10#''${t#*[.,]} / 1000 ))"
    }
    _ixActFmt() {
      printf '%d.%02ds' "$(( $1 / 1000 ))" "$(( $1 % 1000 / 10 ))"
    }
    _ixActStart=$(_ixActNowMs)
    _ixActPrevTs=$_ixActStart
    _ixActPrevName=""
    declare -A _ixActDurations
    # Keep the original under a new name, then wrap it. Only the
    # "Activating %s" header participates in timing; other _iNote messages
    # pass straight through.
    eval "_ixActOrigINote() $(declare -f _iNote | tail -n +2)"
    _ixActLap() {
      local now
      now=$(_ixActNowMs)
      if [[ -n "$_ixActPrevName" ]]; then
        _ixActDurations[$_ixActPrevName]=$(( now - _ixActPrevTs ))
        printf '  %s\n' "$(_ixActFmt $(( now - _ixActPrevTs )))"
      fi
      _ixActPrevTs=$now
      _ixActPrevName=$1
    }
    _iNote() {
      if [[ "''${1:-}" == "Activating %s" && $# -ge 2 ]]; then
        _ixActLap "$2"
      fi
      _ixActOrigINote "$@"
    }
    # Extract the command body of the already-armed EXIT trap so the summary
    # can chain to it; assumes the home-manager trap body itself contains no
    # single quotes (true of the gcroot cleanup it arms today).
    _ixActPrevExitCmd=$(trap -p EXIT)
    _ixActPrevExitCmd=''${_ixActPrevExitCmd#trap -- \'}
    _ixActPrevExitCmd=''${_ixActPrevExitCmd%\' EXIT}
    _ixActSummary() {
      if [[ -n "''${_ixActStart:-}" ]]; then
        _ixActLap end
        echo "Slowest activation steps (total $(_ixActFmt $(( $(_ixActNowMs) - _ixActStart )))):"
        for _ixActName in "''${!_ixActDurations[@]}"; do
          printf '%s %s\n' "''${_ixActDurations[$_ixActName]}" "$_ixActName"
        done | sort -rn | head -n 8 | while read -r _ixActMs _ixActStepName; do
          printf '%9s  %s\n' "$(_ixActFmt "$_ixActMs")" "$_ixActStepName"
        done
      fi
      eval "$_ixActPrevExitCmd"
    }
    trap '_ixActSummary' EXIT
  '';
}
