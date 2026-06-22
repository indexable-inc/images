# Filesystem Bench

Small benchmark for comparing the filesystem a VM sees.

Run it inside the VM against the real target path. Compare that result with
`/tmp` or another normal disk-backed directory on the same VM.

## Run

```sh
nix run github:indexable-inc/index#bench-filesystem -- --target /path/to/vcfs
```

For a local checkout:

```sh
nix run .#bench-filesystem -- --target /path/to/vcfs
```

For a short sanity check:

```sh
nix run .#bench-filesystem -- --target /path/to/vcfs --quick
```

To compare two file systems, run the same command twice with different targets,
for example `/path/to/vcfs` and `/tmp` or another ext4-backed directory.

## Output

The benchmark reports:

- Sequential read and write throughput using 1 MiB blocks.
- Random read and write IOPS using 4 KiB blocks.
- Create, stat, and delete rates for many tiny files.

Use `--json` when collecting results:

```sh
nix run .#bench-filesystem -- --target /path/to/vcfs --json > vcfs.json
```

Treat this as a relative measurement: same VM, same benchmark parameters,
different target directories or different VCFS builds.
