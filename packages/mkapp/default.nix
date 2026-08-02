# mkapp: scaffold a Svelte 5 + Vite + TypeScript app an AI agent serves with
# the kernel's Serve.app (index#4015). The template carries a durable rune
# store (state survives hot reloads and full reloads), the vendored
# shadcn-svelte component registry on Tailwind v4, a self-narrating
# AgentStatus strip, and a staging/ tree the serve gate typechecks and
# promotes into the live src/.
#
# The template ships inside the package and the CLI only copies files, so
# scaffolding needs no network; dependencies install in the scaffolded app
# (`npm install`). The shadcn theme is generated here, at build time, from the
# ghostty palettes so the operator's terminal and the scaffolded apps share
# one source of color truth (index#4064); running the CLI straight out of a
# checkout, where that build never happened, generates the same block into the
# scaffold instead (index#4288). The launcher is the Node script
# itself behind a store shebang: no shell wrapper (shell-allowlist.txt only
# shrinks).
{
  ix,
  lib,
  pkgs,
}: let
  node = lib.getExe pkgs.nodejs_22;
  themes = ix.paths.modules + "/home/ghostty/themes";

  mkapp =
    pkgs.runCommand "mkapp" {
      strictDeps = true;
      passthru.tests = {inherit scaffold;};
      meta = {
        description = "Scaffold a Svelte 5 + Vite app with a durable store, shadcn-svelte UI, and a check-gated staging tree";
        mainProgram = "mkapp";
      };
    } ''
      mkdir -p "$out/bin" "$out/libexec/mkapp"
      cp -R ${./template} "$out/libexec/mkapp/template"
      chmod -R u+w "$out/libexec/mkapp/template"
      ${node} ${./generate-theme.mjs} \
        ${themes + "/custom-light"} \
        ${themes + "/custom-dark"} \
        > "$out/libexec/mkapp/template/src/lib/theme.css"
      {
        printf '#!%s\n' "${node}"
        cat ${./cli.mjs}
      } > "$out/libexec/mkapp/cli.mjs"
      chmod +x "$out/libexec/mkapp/cli.mjs"
      ln -s "$out/libexec/mkapp/cli.mjs" "$out/bin/mkapp"
    '';

  # Both ways of running the CLI end in an app that renders, and neither can
  # hand back a path to one that does not. A CSS @import is resolved by nothing
  # between scaffold and first paint -- svelte-check ignores it, tsc never sees
  # it -- so a template referencing a file it does not ship used to survive
  # scaffold, typecheck and serve, and fail as a Vite overlay on a blank page
  # (index#4288). The store path and the checkout path differ in exactly the
  # file that broke, so both are scaffolded here.
  scaffold = pkgs.runCommand "mkapp-scaffold" {strictDeps = true;} ''
    # A checkout as the CLI sees one: cli.mjs and its generator in
    # packages/mkapp/ beside an ungenerated template, palettes two levels up.
    mkdir -p checkout/packages/mkapp checkout/modules/home/ghostty/themes
    cp -R ${./template} checkout/packages/mkapp/template
    cp ${./cli.mjs} checkout/packages/mkapp/cli.mjs
    cp ${./generate-theme.mjs} checkout/packages/mkapp/generate-theme.mjs
    cp ${themes + "/custom-light"} checkout/modules/home/ghostty/themes/custom-light
    cp ${themes + "/custom-dark"} checkout/modules/home/ghostty/themes/custom-dark
    chmod -R u+w checkout

    ${node} checkout/packages/mkapp/cli.mjs "$PWD/from-checkout" > checkout-path.txt
    [ "$(cat checkout-path.txt)" = "$PWD/from-checkout" ]
    "${mkapp}/bin/mkapp" "$PWD/from-store" > store-path.txt
    [ "$(cat store-path.txt)" = "$PWD/from-store" ]

    # src/ is what vite serves and staging/ is what the serve gate promotes over
    # it, so a stylesheet in one and not the other breaks the app on promotion.
    # The two scaffold paths run one generator over one pair of palettes, so
    # their output is comparable byte for byte rather than merely non-empty.
    for app in from-checkout from-store; do
      for tree in src staging; do
        cmp "$app/$tree/lib/theme.css" \
          "${mkapp}/libexec/mkapp/template/src/lib/theme.css"
      done
    done

    # Take the generator away and the checkout can no longer produce the file
    # app.css imports. That is the state the CLI must refuse in, naming the
    # import, rather than printing a path to a scaffold that cannot render.
    rm checkout/packages/mkapp/generate-theme.mjs
    if ${node} checkout/packages/mkapp/cli.mjs "$PWD/refused" \
      > refused.out 2> refused.err; then
      echo "expected a non-zero exit with no theme.css generator" >&2
      exit 1
    fi
    [ ! -s refused.out ]
    grep -q "src/app.css imports './lib/theme.css'" refused.err

    touch "$out"
  '';
in
  mkapp
