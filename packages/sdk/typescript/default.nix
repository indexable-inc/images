{
  ix,
  lib,
  runCommand,
  nodejs_22,
  typescript,
}: let
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./package.json
      ./src
    ];
  };
  # `allowVendoredUnfree` strips the honest `meta.license` tag below so the
  # per-system flake package set (evaluated without `allowUnfree`) can build
  # this gate; see lib/util/vendored-unfree.nix.
  check = ix.allowVendoredUnfree (
    runCommand "ix-sdk-typescript-typecheck"
    {
      inherit src;
      nativeBuildInputs = [
        nodejs_22
        typescript
      ];
      strictDeps = true;
      passthru.tests.typecheck = check;
      meta = {
        description = "TypeScript type check for the public @indexable/sdk sources";
        homepage = "https://github.com/indexable-inc/index";
        # Stripped by the `ix.allowVendoredUnfree` wrapping above; the tag
        # stays honest (packages/sdk/typescript/LICENSE supersedes the root
        # MIT license) without blocking the flake package set.
        license = lib.licenses.unfree;
      };
    }
    ''
      # The TS SDK is published straight from src, so this gate catches drift that
      # previously reached npm without any Nix check.
      cp -R "$src"/. .
      chmod -R u+w .

      # src/index.ts imports the wasm-bindgen bundle from ../dist/ix_sdk.js. Its
      # real .d.ts is emitted by the dist build in the ix monorepo
      # (crates/ix/sdk-wasm) and does not exist in this repo, so the gate
      # declares that boundary loosely (explicit any) instead of hand-writing
      # signatures that would drift. Strictness comes from what this repo owns:
      # src/index.ts's own logic and the src/native.d.ts N-API surface.
      mkdir -p dist
      cat > dist/ix_sdk.d.ts <<'EOF'
      declare class Loose {
        [key: string]: any
        constructor(...args: any[])
      }
      export {
        Loose as Branch,
        Loose as Client,
        Loose as FsHandle,
        Loose as SecretsHandle,
        Loose as ShellSession,
        Loose as StreamConnection,
        Loose as VmStatusStream,
      }
      export declare const Region: any
      export type Region = any
      declare function init(...args: any[]): Promise<any>
      export default init
      EOF

      cat > tsconfig.json <<'EOF'
      {
        "compilerOptions": {
          "lib": ["ES2024", "ESNext.Disposable", "DOM"],
          "module": "NodeNext",
          "moduleResolution": "NodeNext",
          "noEmit": true,
          "noImplicitOverride": true,
          "noImplicitReturns": true,
          "noUnusedLocals": true,
          "noUnusedParameters": true,
          "exactOptionalPropertyTypes": true,
          "noUncheckedIndexedAccess": true,
          "strict": true,
          "target": "ES2024",
          "verbatimModuleSyntax": true
        },
        "include": ["src/**/*.ts"]
      }
      EOF

      ${lib.getExe' typescript "tsc"} --noEmit --project tsconfig.json
      touch "$out"
    ''
  );
in
  check
