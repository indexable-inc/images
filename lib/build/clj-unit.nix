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

  # Clojure munges a namespace into a path: dots become directories and
  # hyphens become underscores. `com.example.todo-app.model.tab-state` is
  # `com/example/todo_app/model/tab_state`.
  namespacePath = namespace:
    lib.replaceStrings ["." "-"] ["/" "_"] namespace;

  # The namespace graph, rendered from the source by IFD. Rendering reads
  # only the `ns` forms, so it is cheap; the units it plans are what cost.
  renderGraph = {
    pname,
    src,
    sourceRoots,
  }:
    lib.importJSON (
      pkgs.runCommand "${pname}-clj-graph.json" {
        nativeBuildInputs = [nix-clj-unit];
        strictDeps = true;
      } ''
        # Render from inside the source so the graph records paths relative
        # to it. Absolute store paths in the JSON would give the string a
        # context that no derivation attribute is allowed to carry.
        cd ${src}
        nix-clj-unit render \
          ${concatStringsSep " " (map (dir: "--src ${dir}") sourceRoots)} \
          --out "$out"
      ''
    );

  /**
  Compile one namespace.

  `depUnits` are the compiled outputs of the namespaces this one requires,
  already built. `classpathJars` is the dependency-library classpath from
  clj-lock.nix. The source tree assembled here holds exactly one file.
  */
  mkUnit = {
    pname,
    classpathJars,
    src,
  }: namespace: node: depUnits:
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
      } "$unitSource/${namespacePath namespace}.clj"

      # clj-lock's classpath is a file, not a string: the git half of it is
      # only knowable after reading each library's deps.edn inside the
      # fetched checkout, which happens at build time.
      #
      # $out is on the classpath because Clojure loads back the classes it
      # has just emitted while compiling the rest of the namespace.
      classpath="$(cat ${classpathJars})${lib.optionalString (depUnits != []) ":"}${concatStringsSep ":" depUnits}:$unitSource:$out"

      java -cp "$classpath" clojure.main \
        -e "(binding [*compile-path* \"$out\"] (compile '${namespace}))"

      if [ -z "$(find "$out" -name '*.class' -print -quit)" ]; then
        echo "clj-unit: compiling ${namespace} produced no class files" >&2
        exit 1
      fi
    '';
in {
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
    meta ? {},
  }: let
    graph = renderGraph {inherit pname src sourceRoots;};

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
    units =
      lib.mapAttrs (
        namespace: node:
          mkUnit {inherit pname classpathJars src;}
          namespace
          node
          (map (required: units.${required}) closures.${namespace})
      )
      graph.namespaces;

    unitOutputs = map (namespace: units.${namespace}) (attrNames graph.namespaces);
    resources = map (dir: "${src}/${dir}") resourceRoots;
    # The dependency classpath arrives as a file, so the runtime classpath is
    # assembled in a derivation rather than as a Nix string. The launcher
    # embeds this file's contents at compile time, which keeps the classpath
    # out of the runtime and makes a missing entry a build failure.
    runtimeClasspathFile = pkgs.runCommand "${pname}-runtime-classpath" {strictDeps = true;} ''
      printf '%s:%s' \
        "$(cat ${classpathJars})" \
        ${lib.escapeShellArg (concatStringsSep ":" (unitOutputs ++ resources ++ extraClasspath))} \
        > "$out"
    '';
    # Clojure munges the namespace into the generated class name the same way
    # it munges the file path, so `:gen-class` on `com.example.reading-list`
    # emits `com.example.reading_list`.
    mainClass = lib.replaceStrings ["-"] ["_"] mainNamespace;
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
      };
    };
}
