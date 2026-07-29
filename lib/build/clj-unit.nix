/**
clj-unit: per-namespace Clojure AOT compilation, one Nix derivation per
namespace. The Clojure analog of [`lib/rust/cargo-unit.nix`](../rust/cargo-unit.nix)
(one derivation per rustc invocation) and
[`lib/kernel/kbuild-unit.nix`](../kernel/kbuild-unit.nix) (one per C
translation unit), and it borrows the same two-stage shape both use: a
render step turns the native build's own graph into data, and Nix builds
one content-addressed derivation per node of that data.

The graph comes from the source, not from a lock file. Every Clojure file
opens with an `ns` form naming what it requires, and `nix-clj-unit render`
transcribes those edges into JSON (IFD). Dependency *libraries* are a
separate concern and arrive as jars through
[`./clj-lock.nix`](./clj-lock.nix); only the application's own namespaces
become units, because only they are compiled.

# Why each unit hides its dependencies' source

A unit's classpath carries its dependencies' compiled output and its own
single `.clj`, and never a dependency's `.clj`. This is load-bearing, not
tidiness. `clojure.lang.RT/load` prefers a `.class` over its `.clj` only
when the class file's mtime is STRICTLY greater. Nix normalizes every
store mtime to 1, so a visible dependency source ties, loses, and -- with
`*compile-files*` bound true during `compile` -- falls through to a
recompile rather than a load. Measured on a three-namespace fixture:

    equal (nix normalized, both 1970-01-01 00:00:01)
      -> dependency classes leaked into the unit's output: 2
    class strictly newer by 1s
      -> dependency classes leaked into the unit's output: 0
    class older
      -> dependency classes leaked into the unit's output: 2

Classpath ORDER does not fix it; the loader resolves both URLs and
compares regardless of order. Touching class files to a later mtime does
fix it, and is worse: it makes an accidental whole-closure recompile
invisible instead of impossible.

# Why the units are content-addressed

`contentAddressed = true`, matching cargo-unit's default, buys early
cutoff. A comment-only edit to a leaf namespace changes that unit's input
but usually not its output bytes, and a CA output that resolves to the
same store path leaves every dependent unit's input untouched, so the
rebuild stops at the leaf instead of walking the whole dependent cone.
Without it a leaf edit rebuilds every namespace downstream of it whether
or not anything downstream could observe a difference, which is most of
the value of having a graph at all.
*/
{
  lib,
  pkgs,
  jdk,
  nix-clj-unit,
  writeRustApplication,
}: let
  inherit (builtins) attrNames concatStringsSep;

  # Clojure munges namespace punctuation into generated JVM class names.
  # Resource paths only need dot -> slash and hyphen -> underscore, but the
  # `:gen-class` launcher name must use the complete clojure.core/munge table.
  namespacePath = namespace:
    lib.replaceStrings ["." "-"] ["/" "_"] namespace;
  mungeNamespace = namespace:
    lib.replaceStrings
    ["-" "+" "?" "!" "*" "/" "%" "&" "=" ">" "<" ":" "#" "@" "~" "^" "|" "{" "}" "[" "]" "\\" "\""]
    ["_" "_PLUS_" "_QMARK_" "_BANG_" "_STAR_" "_SLASH_" "_PERCENT_" "_AMPERSAND_" "_EQ_" "_GT_" "_LT_" "_COLON_" "_SHARP_" "_CIRCA_" "_TILDE_" "_CARET_" "_BAR_" "_LBRACE_" "_RBRACE_" "_LBRACK_" "_RBRACK_" "_BSLASH_" "_DOUBLEQUOTE_"]
    namespace;

  # `.clj` or `.cljc`, taken from the rendered graph rather than assumed. A
  # `.cljc` namespace copied to a `.clj` name throws `Conditional read not
  # allowed` from a filename that exists nowhere in the tree.
  sourceExtension = file:
    if lib.hasSuffix ".cljc" file
    then ".cljc"
    else ".clj";

  # The namespace graph, rendered from the source by IFD. Rendering reads
  # only the `ns` forms, so it is cheap; the units it plans are what cost.
  # Returns the derivation, not the parsed graph: the caller imports it, and
  # exposes it through `passthru` so the source-independence gate in tests/
  # can inspect this derivation's builder text like any other.
  renderGraph = {
    pname,
    src,
    sourceRoots,
  }:
    pkgs.runCommand "${pname}-clj-graph.json" {
      # The one derivation here that MUST read the whole source: it walks the
      # tree for `ns` forms. Content-addressed so that reading it all does not
      # propagate: an edit that leaves every `ns` form alone renders identical
      # JSON, resolves to the same output, and no unit sees a changed input.
      # This is the cheap version of what cargo-unit gets from planning against
      # a stubbed source tree.
      __contentAddressed = true;
      outputHashAlgo = "sha256";
      outputHashMode = "flat";
      nativeBuildInputs = [nix-clj-unit];
      strictDeps = true;
    } ''
      # Render from inside the source so the graph records paths relative
      # to it. Absolute store paths in the JSON would give the string a
      # context that no derivation attribute is allowed to carry.
      cd ${src}
      nix-clj-unit render \
        ${concatStringsSep " " (map (dir: "--src ${lib.escapeShellArg dir}") sourceRoots)} \
        --out "$out"
    '';

  /**
  Compile one namespace.

  `depUnits` are the compiled outputs of the namespaces this one requires,
  already built. `classpathJars` is the dependency-library classpath from
  clj-lock.nix. The source tree assembled here holds exactly one file.
  */
  mkUnit = {
    pname,
    classpathJars,
    compileResources,
    src,
  }: namespace: node: depUnits: let
    applicationClasspath = concatStringsSep ":" (depUnits ++ compileResources);
  in
    pkgs.runCommand "${pname}-clj-${namespace}" {
      __contentAddressed = true;
      outputHashAlgo = "sha256";
      outputHashMode = "recursive";
      nativeBuildInputs = [jdk];
      strictDeps = true;
      passthru = {inherit namespace depUnits;};
    } ''
      mkdir -p "$out"

      # Exactly this namespace's source, and nothing else. Two reasons, and
      # both are load-bearing:
      #
      # 1. A dependency's .clj must not be reachable from here at all. See
      #    the mtime note in this file's header.
      # 2. The unit's Nix input is a store path carved from that one file,
      #    not the package's source tree. Interpolating the source root and
      #    the file path together would make every unit depend on the whole
      #    tree, so an edit to any namespace would rebuild all of them and
      #    the graph would buy nothing. Measured: with the whole-tree input,
      #    one comment added to model/user.clj moved all 16 of todo-app's
      #    unit derivations.
      #
      #    This comment is inside a Nix indented string, so naming that
      #    root with the obvious dollar-brace spelling reintroduces the very
      #    dependency it warns about: a comment is still interpolated.
      unitSource="$PWD/unit-source"
      mkdir -p "$unitSource/$(dirname ${namespacePath namespace})"
      cp ${
        builtins.path {
          name = "clj-source-${namespace}";
          path = src + "/${node.file}";
        }
      } "$unitSource/${namespacePath namespace}${sourceExtension node.file}"

      # clj-lock's classpath is a file, not a string: the git half of it is
      # only knowable after reading each library's deps.edn inside the
      # fetched checkout, which happens at build time.
      #
      # $out is on the classpath because Clojure loads back the classes it
      # has just emitted while compiling the rest of the namespace.
      classpath="${applicationClasspath}${lib.optionalString (applicationClasspath != "") ":"}$unitSource:$out:$(cat ${classpathJars})"

      # Require everything this unit's `ns` form loads, then compile the target.
      # `compile` binds `*compile-files*` true across the transitive load, so
      # compiling straight away makes every library namespace reached from
      # this one compile into `$out` as well: the libraries arrive from jars
      # and git checkouts as source with no class file, or with one at store
      # mtime 1 that ties and loses by the same RT.load rule this file's header
      # describes. Requiring the target itself is also wrong: non-idempotent
      # top-level code would run once in `require` and again in `compile`.
      #
      # So this preloads `node.loads`, not `node.requires`. `requires` is the
      # graph-internal subset that gives this derivation its build edges;
      # preloading only that leaves the external libraries to be reached for the
      # first time by `compile`, which writes their classes here and trips the
      # foreign-class guard below. Measured: `com.example.todo-app.lib.email`
      # emitted `clojure/tools/logging$call_str$fn__333.class`.
      #
      # Measured before this line existed: one unit held 7244 class files of
      # which exactly one was its own, the 16 units totalled 437 MiB against
      # 768 KiB of application bytecode, and the copies were dead anyway,
      # because the JVM skips a class at mtime 1 next to a jar entry dated
      # 2026 and loads the library from source regardless.
      #
      # `require` loads the closure with `*compile-files*` false, so nothing
      # is written. `compile` then loads this namespace's file directly, and
      # its `ns` form's requires are no-ops against `*loaded-libs*`.
      java -cp "$classpath" clojure.main \
        -e "(doseq [dependency '${builtins.toJSON node.loads}] (require dependency)) (binding [*compile-path* \"$out\"] (compile '${namespace}))"

      # The guard has to name this namespace's own class. Checking merely for
      # `*.class` could not fail while the whole library closure was landing
      # here, which is how the recompile above went unnoticed.
      if [ ! -f "$out/${namespacePath namespace}__init.class" ]; then
        echo "clj-unit: compiling ${namespace} produced no ${namespacePath namespace}__init.class" >&2
        exit 1
      fi

      # And nothing but this namespace's own classes. A foreign class here is
      # the whole-closure recompile coming back.
      foreign="$(find "$out" -name '*.class' -not -path "$out/${namespacePath namespace}*" -print -quit)"
      if [ -n "$foreign" ]; then
        echo "clj-unit: compiling ${namespace} also wrote $foreign, so the dependency closure recompiled" >&2
        exit 1
      fi
    '';
in {
  # Exposed for the root eval fixture that locks this table to
  # clojure.core/munge. Not an application-builder API.
  mungeNamespaceForTest = mungeNamespace;

  /**
  Build a Clojure application as one derivation per namespace, plus a
  launcher.

  Arguments:
  - `pname`, `version`: package identity.
  - `src`: the application source root.
  - `sourceRoots`: directories under `src` holding Clojure source, in
    classpath order. Usually `["src"]`.
  - `resourceRoots`: directories under `src` placed on the runtime
    classpath verbatim. Usually `["resources"]`.
  - `mainNamespace`: the namespace carrying `-main`. Must be `:gen-class`.
  - `classpathJars`: the dependency-library classpath, from
    `clj-lock.nix`'s `classpathFor`.
  - `jdk`: the JDK to compile and run with.

  Returns the launcher derivation, with `passthru.units` naming every
  namespace unit so a caller can build or inspect one in isolation.
  */
  buildApplication = {
    pname,
    version,
    src,
    mainNamespace,
    classpathJars,
    sourceRoots ? ["src"],
    resourceRoots ? ["resources"],
    # Store paths appended to the runtime classpath verbatim, for resources
    # a build step generates rather than the source tree carrying (compiled
    # CSS, vendored browser assets). Not visible to the compile units.
    extraClasspath ? [],
    # The project's `:test` alias, as three pieces: a source containing only
    # the test tree, the directories inside it to put on the classpath, and
    # the namespace whose `-main` runs the suite.
    #
    # Tests run from source rather than as units. They are leaves nothing
    # requires, so a unit per test namespace would add derivations and buy no
    # incrementality. They are kept OUT of `src` so that editing a test does
    # not re-run the namespace-graph render.
    testSrc ? null,
    testRoots ? ["test"],
    testNamespace ? null,
    testInputs ? [],
    meta ? {},
  }: let
    graphFile = renderGraph {inherit pname src sourceRoots;};
    graph = lib.importJSON graphFile;

    # Loading a namespace loads its whole require closure, so a unit needs
    # every TRANSITIVE dependency's compiled output on its classpath, not
    # just the ones its own `ns` form names. `app/archive.clj` requires only
    # `lib.middleware` and `lib.ui`, but loading those reaches `routes`, and
    # with `routes__init.class` unresolvable the load falls through to
    # compiling `routes` from a source the unit deliberately cannot see:
    #
    #   Could not locate com/example/todo_app/routes__init.class,
    #   com/example/todo_app/routes.clj or ...cljc on classpath
    #
    # A lazy attrset fixpoint computes the closures. The renderer rejects
    # cycles, so this terminates.
    closures =
      lib.mapAttrs (
        _: node:
          lib.unique (
            lib.concatMap (required: [required] ++ closures.${required}) node.requires
          )
      )
      graph.namespaces;

    # One derivation per namespace, also a lazy fixpoint: selecting one
    # namespace builds only its own cone, never the whole application.
    # Resource roots are needed while compiling too: loading a namespace can
    # read bundled configuration or templates during initialization. Each root
    # is its own content-keyed path, so changing one resource does not make an
    # unrelated source unit depend on the complete application tree.
    resources =
      map (
        dir:
          builtins.path {
            name = "${pname}-${lib.replaceStrings ["/"] ["-"] dir}";
            path = src + "/${dir}";
          }
      )
      resourceRoots;

    units =
      lib.mapAttrs (
        namespace: node:
          mkUnit {
            inherit pname classpathJars src;
            compileResources = resources ++ extraClasspath;
          }
          namespace
          node
          (map (required: units.${required}) closures.${namespace})
      )
      graph.namespaces;

    unitOutputs = map (namespace: units.${namespace}) (attrNames graph.namespaces);
    # The dependency classpath arrives as a file, so the runtime classpath is
    # assembled in a derivation rather than as a Nix string. The launcher
    # embeds this file's contents at compile time, which keeps the classpath
    # out of the runtime and makes a missing entry a build failure.
    runtimeClasspathFile = pkgs.runCommand "${pname}-runtime-classpath" {strictDeps = true;} ''
      printf '%s:%s' \
        ${lib.escapeShellArg (concatStringsSep ":" (unitOutputs ++ resources ++ extraClasspath))} \
        "$(cat ${classpathJars})" \
        > "$out"
    '';
    # Nothing else in the build starts a JVM against the assembled application,
    # so a namespace missing from the graph, or a unit whose classes do not
    # link, could otherwise build green and die at boot.
    smokeCheck =
      pkgs.runCommand "${pname}-smoke" {
        nativeBuildInputs = [jdk];
        strictDeps = true;
      } ''
        java -cp "$(cat ${runtimeClasspathFile})" clojure.main \
          -e "(require '${mainNamespace}) (println \"loaded ${mainNamespace}\")"
        mkdir -p "$out"
      '';
    # Tests read the application's compiled units plus their own source, so
    # they exercise exactly the bytecode the launcher runs.
    testSources = concatStringsSep ":" (map (dir: "${testSrc}/${dir}") testRoots);
    # `:gen-class` uses clojure.core/munge for the generated JVM class name.
    mainClass = mungeNamespace mainNamespace;
  in
    # A compiled launcher, not a shell wrapper: writeShellApplication is
    # banned repo-wide (#3823) and the shell allowlist only shrinks. `exec`
    # replaces this process, so the JVM inherits the pid and systemd's
    # SIGTERM reaches the JVM rather than a wrapper.
    (writeRustApplication pkgs {
      name = pname;
      text = ''
        fn main() {
            use std::os::unix::process::CommandExt;

            let err = std::process::Command::new("${lib.getExe' jdk "java"}")
                .arg("-cp")
                .arg(include_str!("${runtimeClasspathFile}"))
                .arg("${mainClass}")
                .args(std::env::args_os().skip(1))
                .exec();
            eprintln!("${pname}: exec failed: {err}");
            std::process::exit(127);
        }
      '';
      meta = {mainProgram = pname;} // meta;
    })
    // {
      passthru = {
        inherit units graph version;
        smoke = smokeCheck;
        # The project's own suite, so `nix run .#lint`'s sibling gate covers
        # what `clojure -M:test` covers. Without this the tests are assertions
        # nothing runs, which is the same defect as a service
        # module asserted against `/bin/true`.
        tests =
          {
            smoke = smokeCheck;
          }
          // lib.optionalAttrs (testNamespace != null) {
            clojure =
              pkgs.runCommand "${pname}-clojure-tests" {
                nativeBuildInputs = [jdk] ++ testInputs;
                strictDeps = true;
              } ''
                # A writable HOME and cwd: the suites open SQLite files
                # relative to the working directory.
                export HOME="$NIX_BUILD_TOP/home"
                mkdir -p "$HOME" run
                cd run
                java -cp "$(cat ${runtimeClasspathFile}):${testSources}" \
                  clojure.main -m ${testNamespace}
                mkdir -p "$out"
              '';
          };
        # Exposed for the source-independence gate in tests/, which reads the
        # builder text of every derivation in the graph and asserts that
        # neither the package's own source tree nor the repository root
        # appears in it.
        inherit src;
        classpath = classpathJars;
        runtimeClasspath = runtimeClasspathFile;
        graphRender = graphFile;
      };
    };
}
