{
  id = "chrome-vm";
  # macOS host runner (aarch64-darwin): boots the aarch64-linux `chrome-vm-image`
  # guest under vmkit/libkrun and opens the screenshot Chromium took inside it.
  flake.systems = ["aarch64-darwin"];
  packageSet.systems = ["aarch64-darwin"];
}
