# aarch64 NixOS guest for the `chrome-vm` demo: boots headless, runs Chromium
# against a baked proof page, base64s the screenshot over the serial console
# (hvc0, which vmkit captures), then powers off. The host side
# (packages/chrome-vm) decodes the base64 between the markers into a PNG.
#
# The disk is assembled with **systemd-repart** (via the image/repart module),
# NOT `make-disk-image`: repart runs in the build sandbox with no qemu/kvm VM, so
# the image builds on a plain aarch64-linux builder (e.g. hydra's OrbStack remote
# builder, which has no /dev/kvm). libkrun-efi boots OVMF -> systemd-boot (at the
# EFI removable path) -> the UKI in /EFI/Linux. Modelled on nixpkgs'
# nixos/tests/appliance-repart-image.nix.
{
  lib,
  pkgs,
  config,
  modulesPath,
  ...
}:
let
  # A page that proves a real browser + real JS ran in the guest: it prints the
  # Chromium user-agent, a fresh timestamp, and draws a canvas gradient (all via
  # JS at render time), so a blank/placeholder capture is obvious.
  demoPage = pkgs.writeText "chrome-vm-demo.html" ''
    <!doctype html><html><head><meta charset="utf-8"><style>
      body{margin:0;font-family:system-ui,-apple-system,sans-serif;
        background:linear-gradient(135deg,#1a1a2e,#16213e);color:#eee;height:100vh;
        display:flex;align-items:center;justify-content:center}
      .card{background:#0f3460;padding:48px 56px;border-radius:18px;
        box-shadow:0 24px 70px rgba(0,0,0,.55);max-width:780px}
      h1{margin:0 0 14px;font-size:34px}
      p{font-size:18px;line-height:1.5;margin:8px 0}
      .k{color:#e94560;font-weight:700}
      code{background:#000;padding:2px 7px;border-radius:5px;font-size:15px}
    </style></head><body><div class="card">
      <h1>👋 Hello from inside a Linux VM</h1>
      <p>Rendered by <span class="k">headless Chromium</span> in a
         <span class="k">libkrun</span> Linux guest, booted by
         <code>vmkit</code> on a macOS host.</p>
      <p>UA: <code id="ua"></code></p>
      <p>Rendered at: <code id="t"></code></p>
      <canvas id="c" width="700" height="84"></canvas>
      <script>
        document.getElementById('ua').textContent = navigator.userAgent;
        document.getElementById('t').textContent = new Date().toISOString();
        var x = document.getElementById('c').getContext('2d');
        var g = x.createLinearGradient(0, 0, 700, 0);
        g.addColorStop(0, '#e94560'); g.addColorStop(1, '#53a8b6');
        x.fillStyle = g; x.fillRect(0, 0, 700, 84);
        x.fillStyle = '#fff'; x.font = '26px system-ui, sans-serif';
        x.fillText('this strip was drawn by JS in the guest', 22, 52);
      </script>
    </div></body></html>
  '';
in
{
  imports = [ "${modulesPath}/image/repart.nix" ];

  # Boot path: OVMF (libkrun-efi) -> systemd-boot (EFI removable path) -> UKI.
  # We place the bootloader + UKI manually via repart, so disable grub.
  boot = {
    loader.grub.enable = false;
    # `loglevel=0` keeps kernel printk OFF the console (hvc0): the screenshot
    # comes back as one long base64 line on this same console, and a stray printk
    # mid-line (its `[ 1.234] ...` text survives a whitespace strip) would corrupt
    # the decode. Userspace writes (the markers + base64) are unaffected.
    kernelParams = [
      "console=hvc0"
      "loglevel=0"
    ];
    initrd.availableKernelModules = [
      "virtio_pci"
      "virtio_blk"
      "virtio_console"
      "sd_mod"
    ];
  };

  # Root is the repart "root"-typed partition, found by its GPT partition label.
  fileSystems."/" = {
    device = "/dev/disk/by-partlabel/root";
    fsType = "ext4";
  };

  system.image = {
    id = "chrome-vm";
    version = "1";
  };

  image.repart = {
    name = "chrome-vm";
    # OVMF does not work with repart's default 4096-byte sector size.
    sectorSize = 512;
    partitions = {
      "esp" = {
        contents =
          let
            # aarch64-only image (see package.nix), so the EFI arch is fixed.
            # Avoids depending on `config.nixpkgs.hostPlatform` (unset under
            # eval-config with a bare `system`).
            efiArch = "aa64";
          in
          {
            "/EFI/BOOT/BOOT${lib.toUpper efiArch}.EFI".source =
              "${pkgs.systemd}/lib/systemd/boot/efi/systemd-boot${efiArch}.efi";
            "/EFI/Linux/${config.system.boot.loader.ukiFile}".source =
              "${config.system.build.uki}/${config.system.boot.loader.ukiFile}";
            # Auto-boot the single UKI with no menu/delay.
            "/loader/loader.conf".source = pkgs.writeText "loader.conf" "timeout 0\n";
          };
        repartConfig = {
          Type = "esp";
          Format = "vfat";
          # Roomy for the UKI (kernel + initrd) on aarch64.
          SizeMinBytes = "256M";
        };
      };
      "root" = {
        storePaths = [ config.system.build.toplevel ];
        repartConfig = {
          Type = "root";
          Format = "ext4";
          Label = "root";
          Minimize = "guess";
        };
      };
    };
  };

  systemd.services = {
    # The demo: one oneshot that runs at the end of boot, captures, powers off.
    chrome-shot = {
      description = "Headless Chromium screenshot demo (vmkit chrome-vm)";
      wantedBy = [ "multi-user.target" ];
      after = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        # Mirror stdout/stderr to the serial console so the host's --console-file
        # capture sees the markers + base64.
        StandardOutput = "journal+console";
        StandardError = "journal+console";
        TimeoutStartSec = 90;
      };
      path = [
        pkgs.chromium
        pkgs.coreutils
        pkgs.systemd
      ];
      script = ''
        set -u
        # Write straight to the serial console, bypassing systemd's
        # StandardOutput connector: journal+console prefixes every line with
        # "[ts] chrome-shot[pid]: " and wraps it, which would corrupt the base64
        # (its prefix alphanumerics survive any non-base64 strip). /dev/console
        # is hvc0 (console=hvc0), captured raw by vmkit's --console-file.
        exec >/dev/console 2>&1
        # Belt-and-suspenders with loglevel=0: drop the console printk level to 1
        # (emergency only) so no late kernel message can land inside the base64.
        echo 1 >/proc/sys/kernel/printk || true
        export HOME=/tmp
        mkdir -p /tmp/cr
        echo "===VMKIT-CHROME-DEMO==="
        uname -srm
        chromium --version || true
        # `--virtual-time-budget` lets the page's JS (UA/timestamp/canvas) run
        # before the shot; software raster (no GPU) is fine for a screenshot.
        chromium --headless=new --no-sandbox --disable-gpu --hide-scrollbars \
          --user-data-dir=/tmp/cr --window-size=1280,800 \
          --virtual-time-budget=2500 \
          --screenshot=/tmp/shot.png \
          "file://${demoPage}" || echo "CHROMIUM-EXIT=$?"
        if [ -s /tmp/shot.png ]; then
          echo "===VMKIT-SHOT-BEGIN==="
          base64 -w0 /tmp/shot.png
          echo
          echo "===VMKIT-SHOT-END==="
        else
          echo "===VMKIT-NO-SHOT==="
        fi
        # End the VM; vmkit returns when the guest powers off.
        systemctl poweroff --no-block || poweroff -f || true
      '';
    };

    # Don't block boot waiting for a network that the demo never uses.
    NetworkManager-wait-online.enable = lib.mkDefault false;
  };

  # Trim the image: no docs, no GUI, no DHCP wait (the demo is offline).
  documentation.enable = lib.mkDefault false;
  networking.useDHCP = lib.mkDefault false;
  system.stateVersion = "24.11";
}
