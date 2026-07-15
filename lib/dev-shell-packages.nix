{
  packageNames = [
    "alejandra"
    "coreutils"
    "fd"
    "gh"
    "git"
    "graphite-cli"
    "jq"
    "python3"
    "ripgrep"
    "skopeo"
    "tea"
  ];

  # nixpkgs evaluates the wrapper on Linux and its unwrapped package on Darwin.
  unfreePackageNames = [
    "graphite-cli"
    "graphite-cli-unwrapped"
  ];
}
