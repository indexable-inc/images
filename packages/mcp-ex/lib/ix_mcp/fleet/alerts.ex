defmodule IxMcp.Fleet.Alerts do
  @moduledoc """
  The catalog of fleet conditions worth waking an operator for, and the rule
  that decides what gets in.

  ## The rule

  A predicate qualifies only if it is **silent on a healthy day**. Anything
  that fires routinely trains the reader to ignore it, and then it is worse
  than nothing: it occupies the place a real signal would have used. The
  fleet has already produced the proof -- a watchdog that failed 100+
  consecutive times into an open ticket nobody acted on, and a fork sync that
  was red on every run for four days unread. Red was not the missing signal.
  Attention was.

  So every candidate below carries the rate it was measured at, against real
  fleet data in ClickHouse, and the ones that did not qualify are recorded
  with their numbers so nobody proposes them again.

  ## Accepted

  | id | condition | measured healthy-day rate |
  |---|---|---|
  | `kernel_storage` | kernel `alert`/`emerg` naming filesystem or block-device corruption | **1 day in 30** |
  | `ci_oom_success` | OOM kills inside a CI job that reported `success` | **2 days of the 4 that exist** |
  | `oom_burst` | 3+ `CGROUP_OOM` kills of one `node`+`comm` inside an hour | **1 day in 4** (see caveat) |
  | `observability_blind` | the ClickHouse read itself failed | 0 by construction |

  `kernel_storage` is the one that plainly earns its keep. Over the 30 days to
  2026-07-29 the only kernel-level `alert` lines in the entire fleet were four
  XFS metadata-corruption lines on `hil-compute-2`, the cluster leader, on
  2026-07-29 -- ending in `XFS (dm-2): Unmount and run xfs_repair`. Nobody had
  noticed. That is exactly the shape wanted: silent for a month, then once,
  for something real. Filed as ENG-11210. Every other `alert` in the window
  was `sudo: unknown user`, which is a config annoyance on a login host and is
  excluded by requiring the kernel as the source.

  `oom_burst` carries a caveat that must not be lost: **`logs.oom_kills` holds
  only 22 rows, all since 2026-07-26**, so a 30-day rate for it does not
  exist to be measured. Over the four days that do exist it fires once, on
  2026-07-26, when `nix-eval-jobs` was CGROUP_OOM-killed nine times on
  `vin-compute-1` between 03:28 and 05:27 -- a real CI incident that went
  unnoticed for four days. The burst threshold is what makes it rare: bare
  `signal = 'CGROUP_OOM'` fires on **3 of those 4 days** and does not qualify.
  Revisit the threshold once the table has a month of history.

  One more caveat on `oom_burst`: it buckets by `toStartOfHour`, not a sliding
  hour, so a burst straddling a boundary (two kills at 03:59, two at 04:01)
  never reaches `HAVING kills >= 3`. That makes the measured rate a slight
  under-count and the predicate a slight under-detector; both err in the safe
  direction, but read the prose "3+ inside an hour" as "inside one clock hour".

  VM-guest memory pressure is excluded from `oom_burst`, and the way it is
  excluded matters. On 2026-07-29 a VM named `golden` and its paired
  `vhost-NNNNNNN` kernel threads produced 11 kills in five minutes on
  `hil-compute-2`, all at ~2.13 GB: a guest hitting its own memory cap, which
  is a tenant event rather than a fleet fault.

  Filtering that by the VM's name would be useless the first time someone
  boots a differently-named VM. What generalizes is the pairing: the VMM
  process and its `vhost-*` I/O thread are killed in the same second on the
  same host, because they die of the same cgroup limit. So the predicate drops
  any kill sharing a node and a second with a `vhost-*` kill. The tradeoff is
  explicit: a genuine fleet process that happens to die in the same second as
  a VM thread on the same host is missed. That is a narrow coincidence, and it
  is the right side to err on -- the alternative is an operator being paged for
  every tenant that under-provisions a VM.

  ## Rejected, with numbers

  Measured over the 21-30 days to 2026-07-29. None of these is silent on a
  healthy day, because on this fleet there is no healthy day -- 8 to 37
  systemd units sit failed continuously.

  | candidate | measured rate | why rejected |
  |---|---|---|
  | `journald_logs` level=`error` | 43,752-639,661/day, every day | firehose |
  | `journald_logs` level=`crit` | 97-4,814/day, every day | firehose |
  | `systemd_unit_health` `active_state='failed'` | 8-37 distinct units/day, 22 of 22 days | firehose |
  | novel failure onset (failed today, absent prior 7d) | 1-24/day, 28 of 30 days | firehose |
  | `fleet.unit_health_latest` `unhealthy=1` | 0 of 414 rows, ever | flag is dead; ENG-11211 |
  | bare `signal='CGROUP_OOM'` | 3 of the 4 days that exist | superseded by `oom_burst` |

  The `unhealthy` one deserves a note, because it is the trap this module is
  built to avoid. It is a precomputed fleet-health flag that has never once
  been 1, while 15 units are failed as this is written. Anyone who wired an
  alert to it would have built something that reads "healthy" under every
  possible fleet state, and would not have discovered that by watching it stay
  quiet.

  ## Levels

  Each predicate carries an RFC 5424 level so `logging/setLevel` can raise the
  threshold (see `IxMcp.MCP.Server`). Levels are the coarse control; muting a
  single predicate by id is the fine one, and lives in `IxMcp.Fleet.Watch`.
  """

  alias IxMcp.Fleet.ClickHouse

  @typedoc """
  A fired condition. `fingerprint` identifies the condition *instance* and is
  what dedup keys on: stable while the condition merely persists, different
  when something new happens.
  """
  @type hit :: %{
          predicate: String.t(),
          level: String.t(),
          fingerprint: String.t(),
          summary: String.t()
        }

  @typedoc "Per-predicate outcome: what fired, or why we could not tell."
  @type outcome :: {:ok, [hit()]} | {:error, String.t()}

  # Kernel storage faults. Sources are restricted to the kernel so a
  # userspace process logging the word "corruption" cannot fire this, and
  # signatures to filesystem and block-layer damage -- the class where the
  # kernel is telling you the disk lost data, not that an application is sad.
  # Recency is part of the predicate, not the reader's job (ENG-11210). A
  # 24-hour lookback surfaced a five-hour-old finished operation as a live
  # incident, so the window is now the recent past only.
  #
  # `device` is extracted and carried into the notification, because "XFS
  # corruption on hil-compute-2" and "XFS corruption on a detached loopback
  # volume somebody was inspecting" are the same log line and different events.
  #
  # Two suppressions, and the first draft got BOTH wrong in ways only
  # re-measuring caught. Recorded here because the failure mode is the one this
  # module exists to prevent:
  #
  #  * The device exclusion originally covered `dm-` as well as `loop`/`nbd`.
  #    But `dm-` is device-mapper -- LVM and LUKS -- which is exactly where a
  #    host root filesystem lives, and the ENG-11210 lines were on `dm-2`. So
  #    the predicate matched ZERO rows in 30 days: it had been made incapable
  #    of firing on the only event it was ever built for, while its
  #    measured-rate table still claimed "1 day in 30". That is the
  #    `unhealthy=1` dead-flag trap from the Rejected table below,
  #    self-inflicted, and it survived because the suppression was measured
  #    against the false alarm and never against the predicate's remaining
  #    reach. Only `loop` and `nbd` are excluded now -- a disk image and a
  #    network block device are never this host's root storage.
  #
  #  * The `no-recovery` suppression matched `(node_id, device)` using a
  #    different regex on each side, and the outer extraction returns EMPTY for
  #    three of the four signatures (`EXT4-fs error (device sda1)` contains a
  #    space, which `[a-z0-9-]+` rejects). Pairing `(node, '')` against
  #    `(node, '')` turned the exclusion into a host-wide wildcard: any process
  #    logging the phrase "no-recovery mode" silenced every storage alert on
  #    that host for 15 minutes. Not hypothetical --
  #    `ix-system-clickhouse.service` logs it at info level whenever a query
  #    containing the phrase errors, which is to say whenever this file's own
  #    SQL comes back in an exception. The CTE now demands a kernel source and
  #    a non-empty device, and both sides use one expression.
  @device_re "'\\\\(([a-z0-9]+[0-9-]*)\\\\)'"

  @kernel_storage_sql """
  WITH deliberate AS (
    SELECT DISTINCT node_id, extract(message, #{@device_re}) AS device
    FROM logs.journald_logs
    WHERE timestamp > now() - INTERVAL 30 MINUTE
      AND systemd_unit = ''
      AND message LIKE '%no-recovery mode%'
      AND extract(message, #{@device_re}) != ''
  )
  SELECT node_id, level, message, extract(message, #{@device_re}) AS device
  FROM logs.journald_logs
  WHERE timestamp > now() - INTERVAL 15 MINUTE
    AND level IN ('alert', 'emerg')
    AND systemd_unit = ''
    AND (
      match(message, '(?i)(metadata (CRC|I/O) error|Unmount and run xfs_repair)')
      OR match(message, '(?i)(EXT4-fs error|Remounting filesystem read-only)')
      OR match(message, '(?i)(I/O error, dev [a-z0-9]+, sector)')
      OR match(message, '(?i)md/raid[0-9]*: (Disk failure|too many failures)')
    )
    -- Images and network block devices, never host root storage. dm-* is
    -- deliberately NOT in this list; see the comment above.
    AND NOT match(message, '\\\\((loop|nbd)[0-9]+\\\\)')
    -- Only suppressible when a device was actually identified: an empty
    -- extraction must never match an empty CTE row.
    AND NOT (
      extract(message, #{@device_re}) != ''
      AND (node_id, extract(message, #{@device_re})) IN (SELECT node_id, device FROM deliberate)
    )
  ORDER BY timestamp
  LIMIT 200
  """

  # A burst, not a kill. One CGROUP_OOM is a batch job that asked for too
  # much; three of the same process in an hour is something retrying into a
  # wall. vhost-* are per-VM kernel threads and their VMM peers are excluded
  # upstream of this by the caller's own guest filter.
  @oom_burst_sql """
  WITH vm_moments AS (
    SELECT node_id, toStartOfSecond(timestamp) AS sec
    FROM logs.oom_kills
    WHERE timestamp > now() - INTERVAL 24 HOUR AND comm LIKE 'vhost-%'
  )
  SELECT node_id, comm, toStartOfHour(timestamp) AS hour, count() AS kills,
         round(max(rss_bytes) / 1e9, 2) AS peak_rss_gb
  FROM logs.oom_kills
  WHERE timestamp > now() - INTERVAL 24 HOUR
    AND signal = 'CGROUP_OOM'
    AND comm NOT LIKE 'vhost-%'
    AND (node_id, toStartOfSecond(timestamp)) NOT IN (SELECT node_id, sec FROM vm_moments)
  GROUP BY node_id, comm, hour
  HAVING kills >= 3
  ORDER BY hour
  LIMIT 100
  """

  # The predicate the operator named as already justified: a CI job absorbed
  # kernel OOM kills and reported success anyway.
  #
  # `logs.oom_kills` has no cgroup column and `raw_message` carries none
  # either ("Memory cgroup out of memory: Killed process ..." and nothing about
  # which memcg), so "the victim is in an ix-ci-job-* cgroup" is not directly
  # expressible. Correlating on host and time alone is worthless: CI runs on
  # these hosts essentially always, so 21 of 22 kills have a CI job nearby and
  # the test discriminates nothing.
  #
  # What does work is job identity. `metrics.systemd_unit_health` carries the
  # cgroup and the unit name embeds the GitHub job id
  # (ix-ci-job-<repo>-<run>-<job>.service), so a kill inside a unit's observed
  # lifetime can be joined to `kpi.ci_jobs.conclusion`. The signal is then not
  # "an OOM happened" but "an OOM happened inside a job that reported success",
  # which is the thing nobody can currently see.
  #
  # The attribution caveat, stated because it changes how the result reads:
  # several CI units run concurrently on one host, and a kill inside the
  # lifetime of four of them is attributed to all four. So the notification
  # reports the DISTINCT kill count and the candidate job set, never a per-job
  # count -- summing per-job attributions turns 10 real kills into 57.
  @ci_oom_success_sql """
  WITH ci_units AS (
    SELECT node, unit,
           toUInt64OrZero(extract(unit, '-(\\\\d+)\\\\.service$')) AS gh_job_id,
           min(timestamp) AS first_seen, max(timestamp) AS last_seen
    FROM metrics.systemd_unit_health
    WHERE unit LIKE 'ix-ci-job-%' AND timestamp > now() - INTERVAL 6 HOUR
    GROUP BY node, unit, gh_job_id
  ),
  kills AS (
    SELECT timestamp, node_id, comm
    FROM logs.oom_kills
    WHERE timestamp > now() - INTERVAL 2 HOUR
      AND signal = 'CGROUP_OOM'
      AND comm NOT LIKE 'vhost-%'
  )
  SELECT k.node_id AS node_id,
         k.comm AS comm,
         countDistinct(k.timestamp) AS kills,
         groupUniqArray(j.job_name) AS job_names,
         groupUniqArray(u.unit) AS units,
         groupUniqArray(j.conclusion) AS conclusions
  FROM kills AS k
  INNER JOIN ci_units AS u ON u.node = k.node_id
     AND k.timestamp BETWEEN u.first_seen AND u.last_seen
  INNER JOIN kpi.ci_jobs AS j ON j.job_id = u.gh_job_id
  WHERE j.conclusion = 'success'
  GROUP BY node_id, comm
  ORDER BY kills DESC
  LIMIT 20
  """

  @doc "Every predicate id in the catalog, for validating a mute request."
  @spec ids() :: [String.t()]
  def ids, do: ["kernel_storage", "ci_oom_success", "oom_burst", "observability_blind"]

  @doc """
  The level each predicate reports at, as an RFC 5424 name.
  `observability_blind` is a `warning`: not being able to see the fleet is
  serious, but it is usually a laptop off the tailnet rather than an outage,
  and shouting `critical` for that is how the level gets raised past the
  things that matter.
  """
  @spec level(String.t()) :: String.t()
  def level("kernel_storage"), do: "critical"
  # A job that reported green while the kernel killed things inside it is a
  # lie in the CI record, which is worse than a job that failed loudly.
  def level("ci_oom_success"), do: "critical"
  def level("oom_burst"), do: "warning"
  def level("observability_blind"), do: "warning"

  @doc """
  Evaluate every predicate not in `muted`.

  Returns a map of predicate id to `outcome`. A ClickHouse failure surfaces as
  `{:error, reason}` for that predicate and, separately, as an
  `observability_blind` hit -- so a broken read is itself news rather than a
  quiet fleet. `muted` predicates are absent from the result entirely.
  """
  @spec evaluate([String.t()], (String.t() -> {:ok, [ClickHouse.row()]} | {:error, String.t()})) ::
          %{String.t() => outcome()}
  def evaluate(muted \\ [], query_fun \\ &ClickHouse.query/1) do
    reads =
      %{
        "kernel_storage" => {@kernel_storage_sql, &kernel_storage_hit/1},
        "ci_oom_success" => {@ci_oom_success_sql, &ci_oom_success_hit/1},
        "oom_burst" => {@oom_burst_sql, &oom_burst_hit/1}
      }
      |> Map.drop(muted)
      |> Map.new(fn {id, {sql, shape}} -> {id, read(id, sql, shape, query_fun)} end)

    blind = blindness(reads)

    if "observability_blind" in muted,
      do: reads,
      else: Map.put(reads, "observability_blind", blind)
  end

  defp read(id, sql, shape, query_fun) do
    case query_fun.(sql) do
      {:ok, rows} -> {:ok, Enum.map(rows, &Map.put(shape.(&1), :predicate, id))}
      {:error, reason} -> {:error, reason}
    end
  end

  # One hit naming every predicate that could not be read, keyed on the reason
  # so a persistent outage announces once rather than every poll.
  defp blindness(reads) do
    case for({id, {:error, reason}} <- reads, do: {id, reason}) do
      [] ->
        {:ok, []}

      failures ->
        reason = failures |> Enum.map(&elem(&1, 1)) |> Enum.uniq() |> Enum.join("; ")
        which = failures |> Enum.map(&elem(&1, 0)) |> Enum.sort() |> Enum.join(", ")

        {:ok,
         [
           %{
             predicate: "observability_blind",
             level: level("observability_blind"),
             fingerprint: "observability_blind:" <> hash(reason),
             summary:
               "cannot read fleet telemetry (#{which}) -- this is NOT a report of a healthy fleet: #{reason}"
           }
         ]}
    end
  end

  defp kernel_storage_hit(row) do
    node = row["node_id"]
    message = row["message"] || ""

    device =
      case row["device"] do
        d when d in [nil, ""] -> "?"
        d -> d
      end

    %{
      level: level("kernel_storage"),
      # Keyed on the device, not just the host, and deliberately not on the day:
      # a disk still corrupt tomorrow is the same news, and re-announcing it
      # daily is how a channel earns its mute. Clear it with Fleet.forget/1.
      fingerprint: "kernel_storage:#{node}:#{device}:#{hash(normalize(message))}",
      summary: "#{node} device #{device}: #{String.slice(message, 0, 200)}"
    }
  end

  defp ci_oom_success_hit(row) do
    node = row["node_id"]
    comm = row["comm"]
    jobs = row["job_names"] |> List.wrap() |> Enum.reject(&(&1 in [nil, ""])) |> Enum.sort()
    kills = row["kills"]

    named =
      case jobs do
        [] -> "job name unresolved"
        names -> Enum.join(names, ", ")
      end

    %{
      level: level("ci_oom_success"),
      # Keyed on the job set rather than a time bucket: the same jobs still
      # showing kills on the next poll is the same news, a different job is not.
      fingerprint: "ci_oom_success:#{node}:#{comm}:#{hash(Enum.join(jobs, ","))}",
      summary:
        "#{node}: #{kills} CGROUP_OOM kill(s) of #{comm} inside CI job(s) that reported SUCCESS " <>
          "-- #{named} (candidate jobs; concurrent units mean attribution is by unit lifetime, " <>
          "not by cgroup)"
    }
  end

  defp oom_burst_hit(row) do
    node = row["node_id"]
    comm = row["comm"]
    hour = row["hour"]

    %{
      level: level("oom_burst"),
      # The hour bucket IS the event: a fresh burst next hour is genuinely new,
      # while the same burst seen by three polls is one.
      fingerprint: "oom_burst:#{node}:#{comm}:#{hour}",
      summary:
        "#{node}: #{comm} CGROUP_OOM-killed #{row["kills"]}x in the hour from #{hour} " <>
          "(peak RSS #{row["peak_rss_gb"]} GB)"
    }
  end

  # Kernel messages carry addresses and sector numbers that differ between
  # otherwise identical faults; stripping them keeps one fault to one
  # fingerprint instead of one per retry.
  defp normalize(message) do
    Regex.replace(~r/0x[0-9a-f]+|\b\d{4,}\b/i, message, "N")
  end

  defp hash(text) do
    :crypto.hash(:sha256, text) |> Base.encode16(case: :lower) |> String.slice(0, 12)
  end
end
