{ errors }:
let
  /**
    Node.js major → nixpkgs attribute mapping. The explicit list is
    smaller than what nixpkgs ships because end-of-life lines (16, 18)
    are gone from the channel; bump the top entry when a new even-major
    LTS lands.
  */
  nodesFor = pkgs: {
    "latest" = pkgs.nodejs;
    "20" = pkgs.nodejs_20;
    "22" = pkgs.nodejs_22;
    "24" = pkgs.nodejs_24;
    "25" = pkgs.nodejs_25;
  };

in
{
  /**
    Return the Node.js runtime for `version`.

    "node" rather than a generic `runtime` because Bun and Deno are
    siblings, not implementations of the same enum: they have different
    flag surfaces, different built-in module sets, and different
    npm-compatibility stories. Picking `node`, `bun`, or `deno` is a
    different decision than picking a Node major, so each gets its own
    helper.

    Arguments:
    - `pkgs`: nixpkgs instance the Node package comes from.
    - `version`: required, one of `"latest" | "20" | "22" | "24" | "25"`.
      Pin a specific even-major when the build needs stable ABI
      compatibility for native modules.

    Example:
    ```nix
    { pkgs, ix, ... }:
    let node = ix.languages.javascript.node pkgs { version = "22"; };
    in { environment.systemPackages = [ node ]; }
    ```
  */
  node =
    pkgs: args:
    let
      version = errors.requireArg {
        context = "ix.languages.javascript.node";
        inherit args;
        name = "version";
      };
    in
    errors.requireAttr {
      context = "ix.languages.javascript.node: unknown major";
      attrset = nodesFor pkgs;
      key = version;
    };

  /**
    Return the Bun runtime + package manager + bundler.

    Single binary; nixpkgs ships exactly one `pkgs.bun`, so there is no
    version dimension to validate. Bun is also a package manager: the
    repo's `lib/build/js-site.nix` already consumes it for static-site
    builds; this helper exposes the same binary for runtime images that
    run a Bun server.
  */
  bun = pkgs: _: pkgs.bun;

  /**
    Return the Deno runtime.

    Single-binary all-in-one (runtime, package manager, formatter, test
    runner). Deno permission flags are how you bound what the program
    can touch; that is a service-module concern, not a package-selection
    one, so the helper just returns the binary.
  */
  deno = pkgs: _: pkgs.deno;

  /**
    Return the TypeScript compiler (`tsc`).

    Standalone of any JS runtime; ships with its own bundled Node script
    that nixpkgs wraps. Use alongside `node` / `bun` / `deno` when the
    build needs a separate type-check pass that does not depend on the
    runtime's own type-stripping (Node 22+ and Bun strip types at load
    but do not type-check).
  */
  typescript = pkgs: _: pkgs.typescript;

  /**
    Return the TypeScript language server package.

    `pkgs.typescript-language-server` is the LSP server; it wraps
    `tsserver` from the `typescript` package and serves both `.ts` and
    `.js` files. Intended for dev VMs that host an editor.
  */
  languageServer = pkgs: _: pkgs.typescript-language-server;
}
