/**
Turn a clj-nix `deps-lock.json` into Clojure dependencies: a classpath for a
plain `java -cp`, and a full `.m2`/`.gitlibs` cache tree for anything that
wants to run `clojure` itself. Both are assembled from one fetch derivation
per artifact.

Three properties are load-bearing, and the code is shaped around them:

1. **One derivation per artifact.** Every `mvn-deps` entry gets its own
   `fetchurl` and every distinct `git-deps` checkout its own `fetchgit`. Editing
   one hash in a lock rebuilds that one fetch; the other few hundred are
   untouched. There is deliberately no single fixed-output derivation over the
   whole lock, which would turn a one-line lock bump into a full re-download.

2. **Artifact identity, never app identity.** A fetch derivation is a pure
   function of the `(mvn-repo, mvn-path, hash)` triple, its derivation name
   included. Two applications whose locks name the same artifact therefore get
   the same `.drv` and the same store path, and the store holds one copy. This is why
   there is no per-call `fetcherOpts` escape hatch of the kind `uv-lock.nix` and
   `bun-lock.nix` carry: a caller-supplied fetcher flag would make the same
   artifact build differently depending on who asked for it. Add one only
   together with a story for keeping it out of the derivation identity.

3. **Layouts a JVM tool already understands.** `mvn-path` is repository-relative
   by construction, so `.m2/repository/<mvn-path>` is a normal local Maven
   repository. Git dependencies land at `.gitlibs/libs/<lib>/<rev>`, and each
   distinct checkout gets a `.gitlibs/_repos/<git-dir>/` marker directory --
   `tools.gitlibs` reads an existing `config` there as "this bare repo is
   already cloned" and skips the network. The layout is copied from clj-nix's
   own `pkgs/mkDepsCache.nix`, which is the reference for what `clojure` will
   accept.

Where this overlaps [`./gradle-fat-jar.nix`](./gradle-fat-jar.nix), the repo's
other Maven fetcher, it follows it rather than inventing a second convention:
the same `<group>/<artifact>/<version>/<file>` tree of symlinks to individual
`pkgs.fetchurl` outputs, each derivation named after the Maven file. The one
divergence is `pkgs.linkFarm` instead of a hand-written `mkdir -p` / `ln -s`
loop in `runCommand`, which is the same tree from the primitive that already
means it.

# The lock is a download set, not a classpath

`deps-lock.json` records every artifact `clojure -P` *fetched*, which includes
versions that lost dependency resolution. A Biff lock carries twelve versions of
`org/clojure/clojure` (1.10.3 through 1.12.5) and three each of `spec.alpha` and
`core.specs.alpha`. Putting all of them on a classpath would let Clojure 1.10.3
shadow the 1.12.5 the project pinned, so `classpathFor` keeps the newest version
of each `(group, artifact)` coordinate and drops the rest. That is `tools.deps`'
own tiebreak but not its whole algorithm -- it resolves by top proximity first,
newest version second -- so a lock whose top-level `deps.edn` deliberately pins
an *older* version than something transitive requests would resolve differently
here. Nothing in the lock records the pin. Running `clojure -Spath` against
`mkDepsCache`'s tree is the exact answer, at the cost of an IFD and clj-nix's
`git` shim.

The lock says as little about git dependencies: it records the checkout but not
which directories inside it are code, and not the sibling libraries a
`:local/root` reaches. `classpathFor` recovers both by reading the checkout's
own EDN at build time -- see its doc-comment.
*/
{
  lib,
  pkgs,
}: let
  inherit (builtins) dirOf baseNameOf removeAttrs;

  # clj-nix stamps every generated lock with the schema version of the
  # generator that wrote it. Refuse anything else rather than reading a
  # different schema through these field names and producing a plausible but
  # wrong tree. Bump this together with the parsing below after re-reading
  # clj-nix's `pkgs/mkDepsCache.nix`.
  supportedLockVersion = 4;

  # The only `git-deps` fetcher this builder implements. clj-nix also accepts
  # "builtins.fetchTree", which clones during evaluation to reach an ssh-agent
  # for private repositories; that is impure and banned here, so a lock asking
  # for it fails loudly instead of being fetched some other way.
  supportedGitFetcher = "pkgs.fetchgit";

  jsonFormat = pkgs.formats.json {};

  # A Maven repository path is `<group>/<artifact>/<version>/<file>`, so the
  # coordinate and the version are the two directories above the file.
  coordinateOf = path: dirOf (dirOf path);
  versionOf = path: baseNameOf (dirOf path);

  # `mvn-repo` carries a trailing slash and `mvn-path` does not carry a leading
  # one, but the lock format guarantees neither, so normalise both edges.
  artifactUrl = repo: path:
    lib.concatStringsSep "/" [
      (lib.removeSuffix "/" repo)
      (lib.removePrefix "/" path)
    ];

  mvnArtifactFor = entry:
    assert lib.assertMsg (
      entry ? hash
    ) "ix.cljLock: mvn-deps entry `${entry.mvn-path}` has no hash"; {
      inherit (entry) hash;
      path = entry.mvn-path;
      repo = entry.mvn-repo;
      url = artifactUrl entry.mvn-repo entry.mvn-path;
      # Extra repository paths that resolve to the same bytes. A Maven snapshot
      # is published under both a timestamped file name and the `-SNAPSHOT`
      # one; dropping the alias leaves a tree that resolves for a release
      # dependency and fails for a snapshot.
      aliases = lib.optional (entry ? snapshot) "${dirOf entry.mvn-path}/${entry.snapshot}";
    };

  gitLibraryFor = entry: let
    fetcher = entry.fetch or supportedGitFetcher;
  in
    assert lib.assertMsg (
      fetcher == supportedGitFetcher
    ) "ix.cljLock: git-deps entry `${entry.lib}` asks for fetcher `${fetcher}`; only `${supportedGitFetcher}` is supported"; {
      inherit (entry) rev url hash;
      inherit entry;
      library = entry.lib;
      gitDir = entry.git-dir;
      # Key into `gitRepositories`. Several `lib` names routinely resolve to one
      # commit of one repository -- a Biff app names four `com.biffweb/*`
      # libraries that are all subdirectories of a single checkout.
      checkout = "${entry.git-dir}/${entry.rev}";
    };

  gitRepositoryFor = library: {
    inherit (library) url rev hash gitDir;
    # The `lib` field describes one library inside the checkout, not the
    # checkout, so it is dropped from the shared record and from the `_repos`
    # marker below.
    entry = removeAttrs library.entry ["lib"];
    name = lib.strings.sanitizeDerivationName "${baseNameOf library.url}-${library.rev}";
  };

  # Newest version per coordinate, in lock order. See the note on download sets
  # in this file's header for why this filter has to exist at all.
  resolvedJarsFor = artifacts: let
    jars = lib.filter (artifact: lib.hasSuffix ".jar" artifact.path) artifacts;
    newest =
      lib.foldl' (
        chosen: artifact: let
          coordinate = coordinateOf artifact.path;
          incumbent = chosen."${coordinate}" or null;
        in
          chosen
          // {
            "${coordinate}" =
              if incumbent == null || lib.versionOlder (versionOf incumbent.path) (versionOf artifact.path)
              then artifact
              else incumbent;
          }
      ) {}
      jars;
  in
    lib.filter (artifact: newest."${coordinateOf artifact.path}".path == artifact.path) jars;

  # No `name`: `fetchurl` derives it from the URL basename, which is the Maven
  # file name, and which `lib/build/gradle-fat-jar.nix` -- the repo's other
  # Maven fetcher -- already relies on. One convention for both, and still a
  # pure function of the artifact, so property 2 holds.
  fetchMvnArtifact = artifact:
    pkgs.fetchurl {
      inherit (artifact) url hash;
    };

  # No extra arguments: clj-nix records the hash of `fetchgit` at its defaults
  # (submodules fetched, `.git` dropped), so anything added here would have to
  # be matched by whatever regenerated the lock.
  fetchGitRepository = repository:
    pkgs.fetchgit {
      inherit (repository) name url rev hash;
    };

  checkoutsFor = repositories: lib.mapAttrs (_: fetchGitRepository) repositories;

  mavenRepositoryFor = artifacts:
    pkgs.linkFarm "clj-maven-repository" (
      lib.listToAttrs (
        lib.concatMap (
          artifact: let
            src = fetchMvnArtifact artifact;
          in
            map (path: lib.nameValuePair path src) ([artifact.path] ++ artifact.aliases)
        )
        artifacts
      )
    );

  gitLibsFor = {
    libraries,
    checkouts,
  }:
    pkgs.linkFarm "clj-gitlibs" (
      lib.genAttrs' libraries (
        library:
          lib.nameValuePair "${library.library}/${library.rev}" checkouts."${library.checkout}"
      )
    );

  gitReposFor = repositories:
    pkgs.linkFarm "clj-gitlibs-repos" (
      lib.listToAttrs (
        lib.concatMap (repository: [
          # `tools.gitlibs` probes for `config` to decide whether the bare repo
          # already exists; an empty file is enough to keep it off the network.
          (lib.nameValuePair "${repository.gitDir}/config" pkgs.emptyFile)
          # clj-nix's `git` shim answers `rev-parse` and `merge-base` out of
          # these JSON files instead of a real object database.
          (lib.nameValuePair "${repository.gitDir}/revs/${repository.rev}" (
            jsonFormat.generate "clj-git-rev-${repository.rev}" repository.entry
          ))
        ])
        (lib.attrValues repositories)
      )
    );

  # `clojure` reads a user-level `deps.edn` and `tools/tools.edn` from
  # CLJ_CONFIG and creates them when absent, which a sandbox with a read-only
  # HOME cannot do. Empty maps satisfy the read without contributing deps.
  emptyEdn = pkgs.writeText "empty-edn" "{}\n";
  dotClojure = pkgs.linkFarm "clj-config" {
    "deps.edn" = emptyEdn;
    "tools/tools.edn" = emptyEdn;
  };

  self = {
    /**
    Parse a clj-nix `deps-lock.json` into typed artifact records.

    Arguments:
    - `lockPath`: path to a `deps-lock.json`.

    Returns:
    - `lockVersion`: the schema version the lock declares.
    - `mvnArtifacts`: one record per `mvn-deps` entry, in lock order, each with
      `path` (repository-relative), `repo`, `url`, `hash` and `aliases`.
    - `gitLibraries`: one record per `git-deps` entry, in lock order, each with
      `library`, `rev`, `url`, `gitDir`, and `checkout` (its key into
      `gitRepositories`).
    - `gitRepositories`: the distinct checkouts those libraries resolve to,
      keyed by `<git-dir>/<rev>`. Several libraries commonly share one entry.

    Refuses a lock whose `lock-version` is not the supported one, and a lock
    that names the same `mvn-path` twice, since the assembled tree can only
    carry one of them.
    */
    loadLock = lockPath: let
      raw = lib.importJSON lockPath;
      lockVersion = raw.lock-version or 0;
      mvnArtifacts = map mvnArtifactFor (raw.mvn-deps or []);
      gitLibraries = map gitLibraryFor (raw.git-deps or []);
      gitRepositories = lib.genAttrs' gitLibraries (
        library: lib.nameValuePair library.checkout (gitRepositoryFor library)
      );
      distinctPaths = lib.unique (map (artifact: artifact.path) mvnArtifacts);
    in
      assert lib.assertMsg (lockVersion == supportedLockVersion) ''
        ix.cljLock: ${toString lockPath} declares lock-version ${toString lockVersion}, expected ${toString supportedLockVersion}.
        Regenerate it with `nix run github:jlesquembre/clj-nix#deps-lock`.
      '';
      assert lib.assertMsg (
        lib.length distinctPaths == lib.length mvnArtifacts
      ) "ix.cljLock: ${toString lockPath} lists the same mvn-path more than once"; {
        inherit
          lockVersion
          mvnArtifacts
          gitLibraries
          gitRepositories
          ;
      };

    /**
    The dependency-library classpath, as a derivation whose output is a file
    holding one colon-joined string. Use it as
    `java -cp "$(cat ${cljClasspath})"`; the store paths inside the file are
    ordinary references, so depending on the file pulls in every jar.

    It is a derivation rather than a Nix string because the git half cannot be
    known at evaluation time -- see below -- and reading it through IFD would
    make every consumer's evaluation build a checkout.

    Entries are the individual fetch outputs, not paths inside `mkDepsCache`'s
    tree, so a `.pom`-only change in the lock leaves this file byte-identical
    and nothing built against it rebuilds. `clojure.jar` comes through here like
    every other dependency: `pkgs.clojure` ships `clojure-tools.jar` and
    `exec.jar` but not the compiler.

    Git libraries are placed by reading EDN inside the fetched checkout at build
    time, in [`./clj-lock-paths.clj`](./clj-lock-paths.clj). The lock says where
    a git dependency came from and nothing about what inside it is code: the
    `:deps/root` that selects a subdirectory is in the consuming project's
    `deps.edn`, the `:paths` that select source and resource directories are in
    that subdirectory's own `deps.edn`, and a library reached by `:local/root`
    (Biff's `com.biffweb/sqlite` pulls in `../graph` that way) appears in
    neither the lock nor our `deps.edn`. A missing directory or `deps.edn`
    fails the build naming the library, the resolved path and the rev.

    Arguments:
    - `lock`: path to a `deps-lock.json`.
    - `depsEdn`: path to the consuming project's `deps.edn`, which is where
      `:deps/root` lives. Defaults to the `deps.edn` beside the lock.
    - `name`: derivation name.

    Returns a derivation whose output is that one file, with `passthru.lock`
    and `passthru.mvnJars` (the resolved jar derivations, known at eval time).
    */
    classpathFor = {
      lock,
      depsEdn ? dirOf lock + "/deps.edn",
      name ? "clj-classpath",
    }: let
      parsed = self.loadLock lock;
      checkouts = checkoutsFor parsed.gitRepositories;
      mvnJars = map fetchMvnArtifact (resolvedJarsFor parsed.mvnArtifacts);
      plan = jsonFormat.generate "${name}-plan.json" {
        gitLibraries =
          map (library: {
            inherit (library) rev;
            lib = library.library;
            checkout = "${checkouts."${library.checkout}"}";
          })
          parsed.gitLibraries;
        jars = map (jar: "${jar}") mvnJars;
      };
    in
      pkgs.runCommand name {
        __structuredAttrs = true;
        nativeBuildInputs = [pkgs.babashka];
        passthru = {
          inherit mvnJars plan;
          lock = parsed;
        };
      } ''
        # shell
        # Not one pipeline: a pipe would report `paste`'s exit code and turn a
        # failed resolution into an empty classpath.
        bb ${./clj-lock-paths.clj} ${plan} ${depsEdn} > entries
        paste -sd: - < entries | tr -d '\n' > "$out"
      '';

    /**
    Assemble the full dependency cache for one lock.

    Arguments:
    - `lock`: path to a `deps-lock.json`.
    - `name`: derivation name of the assembled tree.

    Returns a derivation whose root holds `.m2/repository`, `.gitlibs/libs`,
    `.gitlibs/_repos` and `.clojure`. It is a tree of symlinks, so it is
    read-only: point a build at it with `HOME=<cache>`,
    `CLJ_CONFIG=<cache>/.clojure`, `GITLIBS=<cache>/.gitlibs`,
    `JAVA_TOOL_OPTIONS=-Duser.home=<cache>` and a writable `CLJ_CACHE`.

    Unlike `classpathFor` this carries the whole lock, `.pom` files included,
    so it is what a real `clojure`, `mvn` or `lein` invocation needs. A build
    that only compiles and runs wants `classpathFor` instead.

    `passthru` carries `lock`, plus `mvnArtifacts` and `gitRepositories` as
    attrsets from artifact key to the individual fetch derivation, so a caller
    or a test can address one artifact's derivation without the tree.
    */
    mkDepsCache = {
      lock,
      name ? "clj-deps-cache",
    }: let
      parsed = self.loadLock lock;
      mvnArtifacts = lib.genAttrs' parsed.mvnArtifacts (
        artifact: lib.nameValuePair artifact.path (fetchMvnArtifact artifact)
      );
      checkouts = checkoutsFor parsed.gitRepositories;
    in
      (pkgs.linkFarm name {
        ".m2/repository" = mavenRepositoryFor parsed.mvnArtifacts;
        ".gitlibs/libs" = gitLibsFor {
          inherit checkouts;
          libraries = parsed.gitLibraries;
        };
        ".gitlibs/_repos" = gitReposFor parsed.gitRepositories;
        ".clojure" = dotClojure;
      })
      .overrideAttrs (previousAttrs: {
        passthru =
          (previousAttrs.passthru or {})
          // {
            inherit mvnArtifacts;
            lock = parsed;
            gitRepositories = checkouts;
          };
        # Every Maven artifact is a prebuilt jar, as `gradle-fat-jar.nix`
        # records for the same reason.
        meta =
          (previousAttrs.meta or {})
          // {
            description = "Clojure dependency cache assembled from a clj-nix deps-lock.json";
            sourceProvenance = [
              lib.sourceTypes.binaryBytecode
              lib.sourceTypes.fromSource
            ];
          };
      });
  };
in
  self
