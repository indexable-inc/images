# Overseer report app: Svelte 5 + TypeScript components compiled by the
# repo's svelte-bundle CLI into one browser bundle, plus the HTML shell the
# workflow tick fills per run (scripts/overseer.sh splices data.json over
# the __OVERSEER_DATA__ marker and copies bundle.js alongside the page).
{pkgs}:
pkgs.runCommand "overseer-report" {
  nativeBuildInputs = [pkgs.svelte-bundle];
} ''
  mkdir -p "$out"
  svelte-bundle ${./app}/App.svelte --minify --out "$out/bundle.js"
  cp ${./template.html} "$out/template.html"
''
