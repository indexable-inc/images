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
# one source of color truth (index#4064). The launcher is the Node script
# itself behind a store shebang: no shell wrapper (shell-allowlist.txt only
# shrinks).
{
  ix,
  lib,
  pkgs,
}:
pkgs.runCommand "mkapp" {
  strictDeps = true;
  meta = {
    description = "Scaffold a Svelte 5 + Vite app with a durable store, shadcn-svelte UI, and a check-gated staging tree";
    mainProgram = "mkapp";
  };
} ''
  mkdir -p "$out/bin" "$out/libexec/mkapp"
  cp -R ${./template} "$out/libexec/mkapp/template"
  chmod -R u+w "$out/libexec/mkapp/template"
  ${lib.getExe pkgs.nodejs_22} ${./generate-theme.mjs} \
    ${ix.paths.modules + "/home/ghostty/themes/custom-light"} \
    ${ix.paths.modules + "/home/ghostty/themes/custom-dark"} \
    > "$out/libexec/mkapp/template/src/lib/theme.css"
  {
    printf '#!%s\n' "${lib.getExe pkgs.nodejs_22}"
    cat ${./cli.mjs}
  } > "$out/libexec/mkapp/cli.mjs"
  chmod +x "$out/libexec/mkapp/cli.mjs"
  ln -s "$out/libexec/mkapp/cli.mjs" "$out/bin/mkapp"
''
