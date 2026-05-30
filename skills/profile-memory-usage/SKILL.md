---
name: profile-memory-usage
description: Profile RSS growth for a running Linux process with bpftrace. Use when debugging memory growth, allocator arena commits, page-fault-driven RSS increases, mmap/brk growth, or deciding whether a process is allocating new virtual memory versus dirtying already-reserved memory.
---

# Profile Memory Usage

Use this skill to explain why a live process RSS is growing without changing the process. Prefer page-fault tracing when the question is "what is becoming resident now?" and use syscall tracing to check whether the process is also creating new mappings.

## Safety

- Keep probes bounded with `interval:s:<seconds> { exit(); }`.
- Use `sudo` for production processes and services owned by another user.
- If `bpftrace` is missing, use `nix run nixpkgs#bpftrace -- ...`.
- On ix production hosts, gather read-only evidence only. Do not copy binaries, change service config, or add logging.
- Record long or incident-relevant commands with `script` under `/tmp/<checkout>/NNN.typescript` and keep the command in a sibling `.cmd` file.

## Baseline

Identify the target PID and capture process/cgroup memory before and after each probe:

```sh
pid=<PID>

awk '
  /^(VmRSS|RssAnon|RssFile|RssShmem|VmSize):/ { print }
' /proc/$pid/status

awk '{
  rest = $0
  sub(/^.*\) /, "", rest)
  split(rest, fields, " ")
  print "minflt=" fields[8], "majflt=" fields[10]
}' /proc/$pid/stat

cat /proc/$pid/cgroup
```

If the process is in a memory cgroup, read `memory.current` for the matching cgroup before and after the trace:

```sh
while IFS=: read -r _controllers _names path; do
  current="/sys/fs/cgroup${path}/memory.current"
  if [ -r "$current" ]; then
    printf '%s ' "$current"
    cat "$current"
  fi
done < /proc/$pid/cgroup
```

Compare deltas, not just absolute values.

## Trace Page Faults

Use page faults for RSS growth. Minor faults show memory becoming resident without disk I/O. Major faults show disk-backed misses. Write faults are the useful subset for demand-zero or copy-on-write pages becoming dirty RSS.

If a probe fails because a kernel field name differs, inspect the available tracepoint fields:

```sh
sudo bpftrace -lv 'tracepoint:exceptions:page_fault_user'
```

Run a stack trace first:

```sh
sudo bpftrace -e '
tracepoint:exceptions:page_fault_user
/pid == PID/
{
  @faults[ustack(12)] = count();
  if (args->error_code & 2) {
    @write_faults[ustack(12)] = count();
  }
}

interval:s:30
{
  exit();
}

END
{
  print(@faults, 40);
  print(@write_faults, 40);
}'
```

Replace `PID` before running. If the output is too large, collect a leaf-symbol view:

```sh
sudo bpftrace -e '
tracepoint:exceptions:page_fault_user
/pid == PID/
{
  @fault_ip[usym(args->ip)] = count();
  if (args->error_code & 2) {
    @write_fault_ip[usym(args->ip)] = count();
  }
}

interval:s:10
{
  exit();
}

END
{
  print(@fault_ip, 30);
  print(@write_fault_ip, 30);
}'
```

Interpretation:

- `VmRSS`, `RssAnon`, cgroup memory, and `minflt` rising together with no `majflt` growth means anonymous memory is being faulted into RSS.
- Top `@write_faults` stacks name the code paths dirtying pages now.
- Allocator frames such as `mi_page_free_list_extend`, `malloc`, `calloc`, or `realloc` at the top usually mean the allocator is committing already-reserved arenas for callers lower in the stack.
- Copy frames such as `memcpy`, `memcmp`, vector growth, string clone, hash table insert, or codec buffer growth usually identify the data structure being materialized.

## Rule Out New Virtual Mappings

If RSS is growing, check whether the process is creating new mappings or just faulting existing address space:

```sh
sudo bpftrace -e '
tracepoint:syscalls:sys_enter_mmap
/pid == PID/
{
  @mmap_bytes[ustack(10)] = sum(args->len);
  @mmap_count[ustack(10)] = count();
}

tracepoint:syscalls:sys_enter_brk
/pid == PID/
{
  @brk[ustack(10)] = count();
}

interval:s:20
{
  exit();
}

END
{
  print(@mmap_count, 30);
  print(@mmap_bytes, 30);
  print(@brk, 30);
}'
```

If this shows little or nothing while page faults and RSS grow, the process is not expanding virtual memory significantly. It is dirtying or touching memory it had already mapped.

## Report Shape

Summarize the result with measured deltas and the top stacks:

```text
RSS:     <before> -> <after>  (<delta>)
RssAnon: <before> -> <after>  (<delta>)
minflt:  <before> -> <after>  (<delta>)
majflt:  <before> -> <after>  (<delta>)

Top write-fault stacks:
1. <stack or leaf>  <count>
2. <stack or leaf>  <count>
3. <stack or leaf>  <count>

Conclusion:
<new mappings | page-fault-driven RSS | disk-backed faults>, caused by <owner/data structure/call path>.
```

Name uncertainty explicitly. Missing symbols, omitted frame pointers, JIT code, or inlined Rust can make a stack partial. The evidence is still useful when the same allocation/copy/data-structure frames dominate both RSS deltas and page-fault counts.
