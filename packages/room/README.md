# room

`room` serves a shared presence room with a Rust WebSocket backend and a Svelte
frontend.

## Run The Packaged Server

```sh
nix run .#room
```

The packaged server listens on `127.0.0.1:8080` by default and serves the built
Svelte assets from the Nix store.

## Run The Checkout Dev Stack

```sh
nix run .#room-dev
```

`room-dev` starts the Rust backend on `127.0.0.1:8080` and the Vite dev server
on `127.0.0.1:5174`. Open the Vite URL for hot reloads; `/ws` is proxied back
to the Rust backend.
