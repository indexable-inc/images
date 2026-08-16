/**
Baseline systemd hardening for long-running network daemons.

Restricts capabilities, devices, kernel surfaces, and namespaces.
Address families stay open enough to accept inbound TCP/UDP and
AF_UNIX. `ProtectSystem = "strict"` makes the entire filesystem
read-only outside of the API filesystems and any state directory the
service declares (`StateDirectory`, `LogsDirectory`,
`CacheDirectory`, `RuntimeDirectory`); every service using this
baseline must declare a `StateDirectory` if it writes to `/var`.

`PrivateUsers = true` does not change who owns that state directory.
systemd gates its id-mapped-mount handling of exec directories on
`DynamicUser=` alone (`exec_directory_is_private()`), so a static
`User=` gets a plain recursive chown of its `StateDirectory` --
including over a tree that already exists owned by some other uid --
and needs no `DynamicUser=` workaround. What a *nested* state path
(`StateDirectory = "svc/leaf"`) costs is the intermediate directory:
systemd mkdirs it root-owned and chowns only the innermost component
(systemd.exec(5)), and `ProtectSystem = "strict"` bind-mounts only the
declared leaf writable, so writes into the parent fail with EROFS.
Declare the flat path and mkdir the subdirectory from `preStart`.
index/tests/hardened-state-directory-vm.nix measures all of this.

Merge into `serviceConfig` and override individual fields per
service as needed.
*/
{
  CapabilityBoundingSet = [""];
  DeviceAllow = [""];
  LockPersonality = true;
  NoNewPrivileges = true;
  PrivateDevices = true;
  PrivateTmp = true;
  PrivateUsers = true;
  ProtectClock = true;
  ProtectControlGroups = true;
  ProtectHome = true;
  ProtectHostname = true;
  ProtectKernelLogs = true;
  ProtectKernelModules = true;
  ProtectKernelTunables = true;
  ProtectProc = "invisible";
  ProtectSystem = "strict";
  RestrictAddressFamilies = [
    "AF_INET"
    "AF_INET6"
    "AF_UNIX"
  ];
  RestrictNamespaces = true;
  RestrictRealtime = true;
  RestrictSUIDSGID = true;
  SystemCallArchitectures = "native";
  UMask = "0077";
}
