# The public site's content and components as a raw-source npm package,
# `@indexable/site` (packages/site/src/lib, whose package.json declares the
# subpath exports). Consumers such as ix.dev compile the .svx/.svelte/.ts
# sources with their own SvelteKit + mdsvex toolchain, so "build" is a plain
# copy into the store; there is deliberately no compile step here.
{
  ix,
  lib,
  stdenvNoCC,
}: let
  libRoot = ix.paths.packagesRoot + "/site/src/lib";
in
  stdenvNoCC.mkDerivation {
    pname = "indexable-site-lib";
    # Kept in lockstep with packages/site/src/lib/package.json.
    version = "0.1.0";
    src = lib.fileset.toSource {
      root = libRoot;
      # Ship what consumers compile; the vitest suites and their fixtures
      # stay app-side in packages/site.
      fileset =
        lib.fileset.difference
        (lib.fileset.fileFilter (file: !lib.hasSuffix ".test.ts" file.name) libRoot)
        (libRoot + "/fixtures");
    };
    dontConfigure = true;
    dontBuild = true;
    installPhase = ''
      # shell
      runHook preInstall
      mkdir -p "$out"
      cp -R ./. "$out/"
      runHook postInstall
    '';
    meta = {
      description = "Raw-source Svelte library for the index public site: updates, plans, stories, philosophy, components, feed builder, and design tokens";
      license = lib.licenses.mit;
    };
  }
