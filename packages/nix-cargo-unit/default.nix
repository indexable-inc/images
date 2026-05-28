{
  ix,
  pkgs,
}:

let
  inherit (pkgs) lib;
  fs = lib.fileset;
  workspaceRoot = ix.rustWorkspace.root;
  workspaceManifest = lib.importTOML (workspaceRoot + "/Cargo.toml");
  workspaceMembers = workspaceManifest.workspace.members;
  workspaceMemberManifests = map (member: workspaceRoot + "/${member}/Cargo.toml") workspaceMembers;
  explicitTargetPaths =
    targetDir: targets: map (target: target.path or "${targetDir}/${target.name}.rs") targets;
  memberTargetStubs =
    member:
    let
      manifest = lib.importTOML (workspaceRoot + "/${member}/Cargo.toml");
      relative = path: "${member}/${path}";
    in
    map relative (
      lib.optional (manifest ? lib) (manifest.lib.path or "src/lib.rs")
      ++ explicitTargetPaths "src/bin" (manifest.bin or [ ])
      ++ explicitTargetPaths "benches" (manifest.bench or [ ])
      ++ explicitTargetPaths "tests" (manifest.test or [ ])
      ++ explicitTargetPaths "examples" (manifest.example or [ ])
      ++ lib.optional (builtins.pathExists (workspaceRoot + "/${member}/src/main.rs")) "src/main.rs"
      ++ lib.optional (builtins.pathExists (workspaceRoot + "/${member}/src/lib.rs")) "src/lib.rs"
    );
  siblingTargetStubs = lib.concatMap (
    member: lib.optionals (member != "packages/nix-cargo-unit") (memberTargetStubs member)
  ) workspaceMembers;

  # nix-cargo-unit bootstraps the unit graph, so it cannot consume
  # ix.cargoUnit. Cargo still needs each workspace member manifest to keep
  # Cargo.lock frozen against the real workspace.
  scopedTrackedSrc = fs.toSource {
    root = workspaceRoot;
    fileset = fs.intersection (fs.gitTracked workspaceRoot) (
      fs.unions (
        [
          (workspaceRoot + "/Cargo.toml")
          (workspaceRoot + "/Cargo.lock")
        ]
        ++ workspaceMemberManifests
        ++ [ ./. ]
      )
    );
  };
  src = pkgs.runCommand "nix-cargo-unit-src" { } ''
    mkdir -p "$out"
    cp -R --no-preserve=mode,ownership ${scopedTrackedSrc}/. "$out"
    chmod -R u+w "$out"
    ${lib.concatMapStringsSep "\n" (
      path: ''install -Dm0644 /dev/null "$out/${path}"''
    ) siblingTargetStubs}
  '';
in
ix.buildRustPackage pkgs {
  pname = "nix-cargo-unit";
  version = "0.1.0";

  inherit src;
  cargoLock.lockFile = ix.rustWorkspace.cargoLock;
  buildAndTestSubdir = "packages/nix-cargo-unit";
  cargoArgs = [
    "-p"
    "nix-cargo-unit"
  ];
  # Root-level policy checks need sibling crate targets, which would
  # reintroduce the full workspace source closure here. The workspace unit
  # graph still runs those policy checks for nix-cargo-unit.
  policy = {
    cargoMachete.enable = false;
    clippy.enable = false;
  };

  meta.mainProgram = "nix-cargo-unit";
}
