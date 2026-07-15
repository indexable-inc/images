{
  lib,
  paths,
}: let
  usersRoot = paths.users;

  directoryNames = root:
    lib.pipe root [
      builtins.readDir
      (lib.filterAttrs (_: type: type == "directory"))
      lib.attrNames
      (lib.sort lib.lessThan)
    ];

  optionalPath = root: rel: attrName:
    lib.optionalAttrs (builtins.pathExists (root + "/${rel}")) {
      ${attrName} = root + "/${rel}";
    };

  repoFor = userName: repoName: let
    root = usersRoot + "/${userName}/${repoName}";
  in
    {
      inherit root;
    }
    // lib.optionalAttrs (builtins.pathExists (root + "/agent-context")) {
      agentContext =
        {
          root = root + "/agent-context";
        }
        // optionalPath (root + "/agent-context") "sections" "sections"
        // optionalPath (root + "/agent-context") "overlays" "overlays";
    }
    // optionalPath root "skills" "skills";

  reposFor = userName:
    lib.genAttrs (directoryNames (usersRoot + "/${userName}")) (
      repoName: repoFor userName repoName
    );
in
  lib.genAttrs (directoryNames usersRoot) reposFor
