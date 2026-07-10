#!/usr/bin/env nu

def fio-summary [scratch: string, name: string] {
  let job = (open ([$scratch $"($name).json"] | path join) | get jobs.0)
  let read_percentiles = ($job.read.clat_ns.percentile? | default {})
  let write_percentiles = ($job.write.clat_ns.percentile? | default {})

  {
    name: $name
    read: {
      iops: ($job.read.iops? | default 0)
      bandwidthBytesPerSecond: ($job.read.bw_bytes? | default 0)
      meanLatencyNs: ($job.read.clat_ns.mean? | default 0)
      p99LatencyNs: ($read_percentiles | get -o "99.000000" | default 0)
    }
    write: {
      iops: ($job.write.iops? | default 0)
      bandwidthBytesPerSecond: ($job.write.bw_bytes? | default 0)
      meanLatencyNs: ($job.write.clat_ns.mean? | default 0)
      p99LatencyNs: ($write_percentiles | get -o "99.000000" | default 0)
    }
  }
}

def run-fio [
  scratch: string
  name: string
  filename: string
  rw: string
  bs: string
  runtime: int
  ramp_time: int
  size: string
  iodepth: int
  json: bool
  --end-fsync
] {
  if not $json {
    print $"running ($name)..."
  }

  let output = ([$scratch $"($name).json"] | path join)
  let base_args = [
    $"--name=($name)"
    $"--directory=($scratch)"
    $"--filename=($filename)"
    $"--output=($output)"
    "--output-format=json"
    "--ioengine=sync"
    $"--iodepth=($iodepth)"
    "--numjobs=1"
    "--thread=1"
    "--group_reporting=1"
    "--time_based=1"
    $"--runtime=($runtime)"
    $"--ramp_time=($ramp_time)"
    $"--size=($size)"
    $"--rw=($rw)"
    $"--bs=($bs)"
  ]
  let args = if $end_fsync { $base_args | append "--end_fsync=1" } else { $base_args }
  ^fio ...$args
}

def sync-path [path: string] {
  try {
    ^sync $path
  } catch {
    ^sync
  }
}

def metadata-bench [scratch: string, phase: string, files: int] {
  let dir = ([$scratch metadata] | path join)
  mkdir $dir
  let start = (date now)

  match $phase {
    create => {
      for i in 1..$files {
        $"ix-bench-($i)\n" | save --force ([$dir $"file-($i)"] | path join)
      }
      sync-path $dir
    }
    stat => {
      ^find $dir -type f -exec stat "{}" "+" | ignore
    }
    delete => {
      ^find $dir -type f -delete
      rmdir $dir
      sync-path $scratch
    }
    _ => {
      error make { msg: $"unknown metadata phase: ($phase)" }
    }
  }

  let seconds = (((date now) - $start) / 1sec)
  {
    name: $"metadata-($phase)"
    files: $files
    seconds: $seconds
    filesPerSecond: (if $seconds == 0 { 0 } else { $files / $seconds })
  }
}

def bytes-mib [bytes: number] {
  $bytes / 1024 / 1024 | math floor
}

def ns-ms [ns: number] {
  $ns / 1000000 | math floor
}

def print-human [result: record] {
  let seq_write = ($result.fio | where name == seq-write | first)
  let rand_write = ($result.fio | where name == rand-write | first)
  let seq_read = ($result.fio | where name == seq-read | first)
  let rand_read = ($result.fio | where name == rand-read | first)
  let metadata_create = ($result.metadata | where name == metadata-create | first)
  let metadata_stat = ($result.metadata | where name == metadata-stat | first)
  let metadata_delete = ($result.metadata | where name == metadata-delete | first)

  print ""
  print "Results"
  print $"  seq-write:  (bytes-mib $seq_write.write.bandwidthBytesPerSecond) MiB/s, p99 (ns-ms $seq_write.write.p99LatencyNs) ms"
  print $"  seq-read:   (bytes-mib $seq_read.read.bandwidthBytesPerSecond) MiB/s, p99 (ns-ms $seq_read.read.p99LatencyNs) ms"
  print $"  rand-write: ($rand_write.write.iops | math floor) IOPS, p99 (ns-ms $rand_write.write.p99LatencyNs) ms"
  print $"  rand-read:  ($rand_read.read.iops | math floor) IOPS, p99 (ns-ms $rand_read.read.p99LatencyNs) ms"
  print $"  create:     ($metadata_create.filesPerSecond | math floor) files/s"
  print $"  stat:       ($metadata_stat.filesPerSecond | math floor) files/s"
  print $"  delete:     ($metadata_delete.filesPerSecond | math floor) files/s"
}

def main [
  --target: string
  --runtime: int = 8
  --ramp-time: int = 1
  --size: string = "256m"
  --files: int = 5000
  --iodepth: int = 1
  --quick
  --json
  --keep
] {
  let target_arg = if ($target | is-empty) { $env.VCFS_BENCH_TARGET? | default "" } else { $target }
  if ($target_arg | is-empty) {
    error make { msg: "missing --target DIR or VCFS_BENCH_TARGET" }
  }
  let target = ($target_arg | path expand)
  if not ($target | path exists) {
    error make { msg: $"target does not exist: ($target)" }
  }
  if (($target | path type) != "dir") {
    error make { msg: $"target is not a directory: ($target)" }
  }

  let parameters = if $quick {
    {
      runtime: 2
      rampTime: 0
      size: "64m"
      files: 1000
      iodepth: $iodepth
    }
  } else {
    {
      runtime: $runtime
      rampTime: $ramp_time
      size: $size
      files: $files
      iodepth: $iodepth
    }
  }
  let scratch = (^mktemp -d $"($target)/.ix-fs-bench.XXXXXX" | str trim)

  if not $json {
    print "ix filesystem benchmark"
    print $"target: ($target)"
    print $"scratch: ($scratch)"
    print $"runtime: ($parameters.runtime)s per fio workload"
    print $"size: ($parameters.size) per fio workload"
    print $"metadata files: ($parameters.files)"
    print ""
  }

  run-fio $scratch seq-write seq-write.dat write 1m $parameters.runtime $parameters.rampTime $parameters.size $parameters.iodepth $json --end-fsync
  run-fio $scratch rand-write rand-write.dat randwrite 4k $parameters.runtime $parameters.rampTime $parameters.size $parameters.iodepth $json --end-fsync

  if not $json {
    print "prefilling read source..."
  }
  ^fio $"--name=prefill-read-source" $"--directory=($scratch)" "--filename=read-source.dat" $"--output=($scratch)/prefill.json" "--output-format=json" "--ioengine=sync" $"--iodepth=($parameters.iodepth)" "--numjobs=1" "--thread=1" "--group_reporting=1" "--rw=write" "--bs=1m" $"--size=($parameters.size)" "--end_fsync=1" | ignore

  run-fio $scratch seq-read read-source.dat read 1m $parameters.runtime $parameters.rampTime $parameters.size $parameters.iodepth $json
  run-fio $scratch rand-read read-source.dat randread 4k $parameters.runtime $parameters.rampTime $parameters.size $parameters.iodepth $json

  if not $json { print "running metadata-create..." }
  let metadata_create = (metadata-bench $scratch create $parameters.files)
  if not $json { print "running metadata-stat..." }
  let metadata_stat = (metadata-bench $scratch stat $parameters.files)
  if not $json { print "running metadata-delete..." }
  let metadata_delete = (metadata-bench $scratch delete $parameters.files)

  let filesystem = try { ^stat -f -c %T $target | str trim } catch { ^stat -f %T $target | str trim }
  let result = {
    generatedAt: (date now | format date "%Y-%m-%dT%H:%M:%SZ")
    target: $target
    scratch: $scratch
    filesystem: $filesystem
    system: (^uname -a | str trim)
    parameters: {
      size: $parameters.size
      runtimeSeconds: $parameters.runtime
      rampTimeSeconds: $parameters.rampTime
      iodepth: $parameters.iodepth
      metadataFiles: $parameters.files
    }
    fio: [
      (fio-summary $scratch seq-write)
      (fio-summary $scratch rand-write)
      (fio-summary $scratch seq-read)
      (fio-summary $scratch rand-read)
    ]
    metadata: [
      $metadata_create
      $metadata_stat
      $metadata_delete
    ]
  }

  if $json {
    print ($result | to json)
  } else {
    print-human $result
  }

  if not $keep {
    rm --recursive --force $scratch
  } else if not $json {
    print ""
    print $"scratch kept at: ($scratch)"
  }
}
