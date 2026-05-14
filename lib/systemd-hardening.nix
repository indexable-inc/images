/**
  Baseline systemd hardening for long-running network daemons.

  Restricts capabilities, devices, kernel surfaces, and namespaces.
  Address families stay open enough to accept inbound TCP/UDP and
  AF_UNIX. Merge into `serviceConfig` and override individual fields per
  service as needed.
*/
{
  CapabilityBoundingSet = [ "" ];
  DeviceAllow = [ "" ];
  LockPersonality = true;
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
