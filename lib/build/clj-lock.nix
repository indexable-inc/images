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

`deps-lock.json` records every artifact `clojure -P` *fetched*, which is a
superset of any one classpath and says almost nothing about how to build one.
A Biff lock carries twelve versions of `org/clojure/clojure` (1.10.3 through
1.12.5) and three each of `spec.alpha` and `core.specs.alpha`, only one of each
of which wins resolution. For git dependencies it records the checkout but not
which directories inside it are code, nor the sibling libraries a `:local/root`
reaches -- Biff's `com.biffweb/sqlite` pulls in `../graph` that way, and nothing
in the lock hints at it.

So nothing here decides what belongs on a classpath. `classpathFor` runs
`tools.deps` against the fetched artifacts, offline, and writes down its answer.

# The classpath is platform-dependent, and should be

`tools.deps` resolves classifier-scoped natives for the platform it runs on, so
the same lock can produce a different classpath per system. It only does so
where such an artifact is in play: `reading-list` resolves byte-identically on
`x86_64-linux` and `aarch64-darwin`, while `todo-app`, which reaches `brotli4j`
through Jetty, does not.

That dependence is the correct behaviour and is strictly safer than a
platform-invariant answer. A lock generated on Linux carries
`brotli4j/native-linux-x86_64` and no `native-osx-aarch64`, so resolving it on
darwin fails the build with

    Error building classpath. The following artifacts could not be resolved:
      com.aayushatharva.brotli4j:native-osx-aarch64:jar:1.23.0 (absent):
      Could not find artifact ... in central (file:///ix-clj-lock-offline)

where a platform-invariant classpath hands the JVM a Linux native it cannot load
and defers the failure to the first request that touches compression. A lock
that does not carry the target platform's artifacts refuses to build for that
platform. Regenerate the lock there, or set `meta.platforms` to the systems its
lock covers.
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

    `tools.deps` does the resolving, not this file. It runs inside the
    derivation against `mkDepsCache`'s tree, with both `:mvn/repos` pointed at
    a `file://` path that cannot exist, so a resolution the lock does not fully
    serve fails here instead of quietly reaching the network for an artifact
    nothing pinned. There is no second implementation and no fallback: a
    resolution failure is a build failure naming the artifact Maven could not
    read.

    Resolution is a build step rather than an eval-time one because it needs
    bytes -- the poms in the local repository, and the `deps.edn` files inside
    the git checkouts. Reading those at eval would mean IFD, so every consumer's
    evaluation would build the whole dependency closure before it could produce
    a derivation.

    The output holds the individual fetch outputs, resolved through the cache's
    symlinks, and never a path into the cache tree itself. The derivation is
    content-addressed for the reason `clj-unit.nix` gives for its units: an edit
    that changes an input without changing the resolved classpath resolves to
    the same output path, so nothing downstream rebuilds. The project's own
    `:paths` come back from `-Spath` as relative entries and are dropped;
    `clj-unit` puts its own source and resource roots on the classpath.
    `clojure.jar` arrives like every other dependency, since `pkgs.clojure`
    ships `clojure-tools.jar` and `exec.jar` but not the compiler.

    Arguments:
    - `lock`: path to a `deps-lock.json`.
    - `depsEdn`: the project `deps.edn` to resolve. Defaults to the one beside
      the lock.
    - `name`: derivation name.

    Returns a derivation whose output is that one file, with `passthru.lock`
    and `passthru.cache`.
    */
    classpathFor = {
      lock,
      depsEdn ? dirOf lock + "/deps.edn",
      name ? "clj-classpath",
    }: let
      parsed = self.loadLock lock;
      # Path names for build-time diagnostics, with their string context
      # discarded. A path interpolated into the builder carries the context of
      # the store path it came from, which inside a flake is the whole
      # repository source, so a message merely NAMING the lock would make this
      # derivation depend on every file in the repo. Measured before the fix:
      # editing one .clj moved all 16 of todo-app's units, because two error
      # strings mentioned these paths.
      #
      # Discarding the context is not enough on its own: the store path is
      # still IN the string, so the builder's bytes change whenever the
      # repository's source hash does, and the derivation moves anyway. The
      # message therefore names the last two segments, which is what a reader
      # needs and is stable across every unrelated edit.
      readable = path: let
        segments = lib.splitString "/" (builtins.unsafeDiscardStringContext (toString path));
        count = builtins.length segments;
      in
        lib.concatStringsSep "/" (lib.sublist (count - 2) 2 segments);
      # Carve `deps.edn` out as a store path of its own. Interpolating the
      # path directly would carry the context of whatever store path it came
      # from, which inside a flake is the whole repository source: an edit
      # anywhere would then move this derivation, and with it every Clojure
      # unit that reads the classpath. `builtins.path` keys the input on this
      # one file's bytes instead. Measured before the fix: editing one .clj
      # moved all 16 of todo-app's units.
      projectDeps = builtins.path {
        name = "${name}-deps.edn";
        path = depsEdn;
      };
      # The default name, so a caller that also asks for `mkDepsCache` on this
      # lock gets the same derivation rather than a second copy of the tree.
      cache = self.mkDepsCache {inherit lock;};
      # Every store path this lock pays for. A resolved classpath entry outside
      # this set is an artifact Maven found somewhere the lock does not
      # describe, which is the one thing the `file://` repositories cannot rule
      # out on their own (the local repository is a real directory).
      fetched = pkgs.writeText "${name}-fetched" (
        lib.concatMapStringsSep "\n" (artifact: "${artifact}") (
          map fetchMvnArtifact parsed.mvnArtifacts
          ++ lib.attrValues (checkoutsFor parsed.gitRepositories)
        )
      );
    in
      pkgs.runCommand name {
        __structuredAttrs = true;
        __contentAddressed = true;
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
        nativeBuildInputs = [pkgs.clojure];
        passthru = {
          inherit cache;
          lock = parsed;
        };
      } ''
        # shell
        # The CLI writes .cpcache beside deps.edn and wants a HOME, so resolve
        # from a writable copy rather than from the store path.
        export HOME="$NIX_BUILD_TOP/home"
        export CLJ_CACHE="$NIX_BUILD_TOP/cpcache"
        export CLJ_CONFIG="$HOME/.clojure"
        export GITLIBS="${cache}/.gitlibs"
        mkdir -p "$HOME" "$CLJ_CACHE" "$NIX_BUILD_TOP/m2" project
        cp -rL ${cache}/.clojure "$CLJ_CONFIG"
        chmod -R u+w "$CLJ_CONFIG"
        cp ${projectDeps} project/deps.edn

        # Maven creates a directory per coordinate it looks up, including ones
        # it will not find. Against the read-only store that surfaces as
        # AccessDeniedException, which buries the real "this artifact is not in
        # the lock" behind a permissions error. Copying the tree is cheap
        # (a few hundred symlinks) and lets the resolver report what it could
        # not read. Copy the CONTENTS: `.m2/repository` is itself a symlink, and
        # copying it as one leaves every later write aimed back at the store.
        cp -R ${cache}/.m2/repository/. "$NIX_BUILD_TOP/m2"
        # Only the directories, and never through a link: `chmod -R` follows
        # symlinks on BSD, which walks straight back into the store.
        find "$NIX_BUILD_TOP/m2" -type d -exec chmod u+w {} +

        (
          cd project
          clojure -Srepro -Spath -Sdeps '{:mvn/local-repo "'"$NIX_BUILD_TOP"'/m2"
                                          :mvn/repos {"central" {:url "file:///ix-clj-lock-offline"}
                                                      "clojars" {:url "file:///ix-clj-lock-offline"}}}'
        ) > resolved

        tr ':' '\n' < resolved > resolved-lines
        : > entries
        while IFS= read -r entry; do
          # A relative entry is one of the project's own :paths.
          case "$entry" in
            /*) ;;
            *) continue ;;
          esac
          target="$(readlink -f "$entry")"
          suffix="''${target#/nix/store/}"
          if [ "$suffix" = "$target" ] || ! grep -qxF "/nix/store/''${suffix%%/*}" ${fetched}; then
            echo "clj-lock: tools.deps resolved $entry to $target, which ${readable lock} did not fetch" >&2
            exit 1
          fi
          printf '%s\n' "$target" >> entries
        done < resolved-lines

        if [ ! -s entries ]; then
          echo "clj-lock: tools.deps resolved an empty classpath from ${readable depsEdn}" >&2
          exit 1
        fi

        # Not one pipeline: a pipe would report `tr`'s exit code.
        paste -sd: - < entries > joined
        tr -d '\n' < joined > "$out"
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
