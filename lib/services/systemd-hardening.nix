/**
  Baseline systemd hardening for long-running network daemons.

  Restricts capabilities, devices, kernel surfaces, and namespaces.
  Address families stay open enough to accept inbound TCP/UDP and
  AF_UNIX. `ProtectSystem = "strict"` makes the entire filesystem
  read-only outside of the API filesystems and any state directory the
  service declares (`StateDirectory`, `LogsDirectory`,
  `CacheDirectory`, `RuntimeDirectory`); every service using this
  baseline must declare a `StateDirectory` if it writes to `/var`.

  Merge into `serviceConfig` and override individual fields per
  service as needed.
*/
{
  CapabilityBoundingSet = [ "" ];
  DeviceAllow = [ "" ];
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
