// loro-crdt is loaded at runtime from esm.sh (WASM-backed; not bundled). Declare
// the surface the dashboard uses so the TypeScript build resolves the URL import.
//
// The dashboard reads the whole oplog: it imports the document, lists every
// change (each carries a millisecond `timestamp` the hub records), and checks
// the document out to a past version to replay it. The aggregator is the only
// editor, so the oplog is a single linear peer history.
declare module 'https://esm.sh/loro-crdt@1' {
  // One operation's address: a peer id and a counter within that peer.
  export interface OpId {
    peer: string;
    counter: number;
  }

  // Metadata for one change (a batch of ops committed together).
  export interface ChangeMeta {
    peer: string;
    counter: number;
    length: number;
    lamport: number;
    timestamp: number;
  }

  export class LoroDoc {
    import(bytes: Uint8Array): void;
    toJSON(): { panes?: Record<string, unknown> };
    // Every change, keyed by peer id. The aggregator is the sole editor, so this
    // is effectively one peer's linear history.
    getAllChanges(): Map<string, ChangeMeta[]>;
    // Move the document view to a past version (detached) or back to the latest.
    checkout(frontiers: OpId[]): void;
    checkoutToLatest(): void;
    isDetached(): boolean;
  }
}
