# `nix run .#chrome-vm`: boot a Linux guest under vmkit/libkrun on the macOS host,
# run headless Chromium inside it against a baked proof page, and open the
# screenshot the guest captured. Self-contained: the guest needs no network, no
# GPU, and no host sharing; the screenshot comes back base64 over the serial
# console (see ../chrome-vm-image/nixos.nix).
#
# The aarch64-linux guest image is built at run time (`nix build` of
# `chrome-vm-image`), which offloads to a linux builder; on hydra that is the
# local OrbStack remote builder. Override the source flake for local testing with
# `IX_CHROME_VM_FLAKE=/path/to/checkout nix run .#chrome-vm`.
{
  writeNushellApplication,
  ix,
  bash,
  gawk,
  coreutils,
  gnugrep,
}:
let
  # The aarch64-darwin vmkit binary (self-signs + re-execs at runtime). Repo
  # crates aren't overlaid into `pkgs`, so reach it through the workspace units
  # the same way packages/mcp does.
  vmkit = ix.rustWorkspace.units.binaries."vmkit";
in
writeNushellApplication {
  name = "chrome-vm";
  runtimeInputs = [
    vmkit
    bash
    gawk
    coreutils
    gnugrep
  ];
  text = ''
    def main [out?: string] {
      let flake = ($env.IX_CHROME_VM_FLAKE? | default "github:indexable-inc/index")
      let outpath = ($out | default $"($env.PWD)/chrome-vm-shot.png")
      let work = (^mktemp -d | str trim)

      print $"chrome-vm: building the aarch64 Linux guest image from ($flake)"
      print "  (first run fetches Chromium's closure; the build offloads to a linux builder)"
      let raw = (
        ^nix build $"($flake)#packages.aarch64-linux.chrome-vm-image"
          --no-link --print-out-paths --accept-flake-config
        | str trim
      )

      # libkrun needs a writable disk; the Nix store image is read-only.
      let disk = $"($work)/disk.raw"
      ^cp $raw $disk
      ^chmod u+w $disk

      let log = $"($work)/console.log"
      print "chrome-vm: booting the Linux guest under vmkit (libkrun-efi)..."
      # The guest screenshots on boot, prints the PNG as base64 over the console,
      # then powers off. vmkit's watchdog exit()s 0 on both a clean poweroff and a
      # timeout, so a nonzero exit here is a real pre-boot libkrun error; swallow
      # it so the diagnostics + cleanup below still run (the PNG check catches it).
      try {
        ^vmkit boot-linux --disk $disk --console-file $log --memory-mib 2048 --cpus 4 --timeout-secs 150
      }

      # Decode the screenshot the guest base64'd between the console markers (one
      # `base64 -w0` line). bash positional args avoid nu<->sh quoting. Swallow a
      # decode error so the diagnostics below run instead of a bare nu abort.
      try {
        ^bash -c 'awk "/===VMKIT-SHOT-BEGIN===/{f=1;next} /===VMKIT-SHOT-END===/{f=0} f" "$1" | tr -d "[:space:]" | base64 -d > "$2"' bash $log $outpath
      }

      # Success only on a real PNG (guards against a truncated/partial decode).
      let ok = if ($outpath | path exists) {
        (open --raw $outpath | first 8) == 0x[89 50 4E 47 0D 0A 1A 0A]
      } else {
        false
      }
      if $ok {
        print ""
        print "chrome-vm: Chromium rendered + screenshotted inside the Linux guest:"
        ^grep -m1 -A4 VMKIT-CHROME-DEMO $log | lines | each {|l| print $"  | ($l)" }
        print $"  -> ($outpath)"
        ^rm -rf $work
        try { ^open $outpath }
      } else {
        print -e "chrome-vm: no valid screenshot captured. last console lines:"
        ^tail -n 40 $log
        ^rm -rf $work
        exit 1
      }
    }
  '';
}
