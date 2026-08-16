#pragma once
///@file

#include "nix/util/types.hh"

#include <optional>
#include <string>

namespace nix {

/**
 * A record of one `nix` command, written while it runs and read back after it
 * has exited.
 *
 * "Invocation" is Bazel's noun for one run of a build tool, kept distinct from
 * the derivation builds inside it; `nix store builds` already owns the word
 * "build" for those. One invocation contains many builds.
 *
 * The record is a directory under `$XDG_STATE_HOME/nix/invocations/<id>/`:
 *
 * - `meta.json`       the command line, cwd, timings and exit status
 * - `events.jsonl`    the `internal-json` event stream, timestamped
 * - `eval-stats.json` the evaluator statistics (`NIX_SHOW_STATS` format)
 *
 * It lives on the client, not the daemon, because only the client sees
 * evaluation, and because the daemon serves several users at once: a
 * machine-wide record directory would show every user the others' builds. The
 * cost is that a derivation another client was already building is recorded
 * here with this invocation's wait for it, not that build's own duration.
 */
namespace invocationRecord {

/**
 * Mint an invocation id, create its record directory, tee the event stream
 * into it and turn on the evaluator counters.
 *
 * Does nothing unless the `invocation-records` experimental feature is
 * enabled, and nothing inside a nested `nix` process (a build hook or a
 * `__build-remote` self-invocation), which would otherwise mint one record per
 * remote build.
 *
 * Call after the command line is parsed, so `--experimental-features` on the
 * command line is in effect, and before any evaluation.
 */
void start(const Strings & argv, bool suppress);

/**
 * Write `meta.json` and print the invocation id.
 *
 * Call after the command's own objects are destroyed, since the evaluator
 * writes its statistics from `~EvalCommand`, and after the exit status is
 * known.
 */
void finish(int exitStatus);

} // namespace invocationRecord

} // namespace nix
