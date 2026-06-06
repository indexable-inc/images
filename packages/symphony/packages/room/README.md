# room

`room` is the Tauri 2.0 desktop client for the multiplayer thread
viewer. It bundles the Svelte 5 UI and connects to a
[`room-server`](../room-server) instance over HTTP + WebSocket.

## Layout

```
packages/room/
├── src/              Svelte 5 frontend (entry: src/main.ts)
├── public/           Static assets
├── index.html        Vite entry HTML
├── package.json      Frontend deps + tauri scripts
├── vite.config.ts    Vite (1420 dev port, /api + /ws proxy)
└── src-tauri/        Tauri shell (standalone Cargo workspace)
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── capabilities/
    └── src/
```

The `src-tauri/` Cargo workspace is intentionally **separate** from
the symphony top-level workspace — Tauri pins its own dep tree and
the desktop app is built locally, not via Nix.

## Develop

```sh
cd packages/room
npm install
npm run tauri:dev          # spawns vite on :1420 and the Tauri window
```

`tauri dev` starts Vite, waits for the dev URL, then launches the
desktop window with hot reload. Closing the window exits the dev
loop.

For pure browser dev (no Tauri window) you can run `npm run dev`
and open http://localhost:1420 in a browser — `/api` and `/ws` are
proxied to `ROOM_BACKEND_URL` (default `http://127.0.0.1:8080`).

## Configure The Backend

In dev, `vite.config.ts` proxies `/api` and `/ws` to the URL in
`ROOM_BACKEND_URL` (default `http://127.0.0.1:8080`).

In a packaged Tauri build the webview origin is `tauri://localhost`,
so there is no Vite proxy. The Svelte side reads
`VITE_ROOM_BACKEND_URL` baked in at build time:

```sh
VITE_ROOM_BACKEND_URL=https://room.ix.dev npm run tauri:build
```

When unset, the bundled app falls back to relative paths, which only
works if you also serve the static SPA from the same origin as the
backend.

## Package

```sh
npm run tauri:build        # produces a .app / .dmg / .deb / .msi
```

First time only: generate icons from a 1024×1024 source PNG:

```sh
npx @tauri-apps/cli icon path/to/source.png
```

Tauri writes the resulting icon set into `src-tauri/icons/`.

## Wire Protocol

See [`../room-server/README.md`](../room-server/README.md) for the
HTTP + WebSocket surface the UI consumes.
