// loro-crdt is loaded at runtime from esm.sh (WASM-backed; not bundled). Declare
// the surface the dashboard uses so the TypeScript build resolves the URL import.
declare module 'https://esm.sh/loro-crdt@1' {
  export class LoroDoc {
    import(bytes: Uint8Array): void;
    toJSON(): { terminals?: Record<string, unknown> };
  }
}
