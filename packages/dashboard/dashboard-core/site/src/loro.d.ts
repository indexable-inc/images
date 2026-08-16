// loro-crdt is loaded at runtime from esm.sh (WASM-backed; not bundled). Declare
// the surface the dashboard uses so the TypeScript build resolves the URL import.
//
// The dashboard both READS and WRITES the document. It imports frames from
// `/events`, lists every change (each carries the millisecond `timestamp` the hub
// stamps via `set_next_commit_timestamp`, not loro's default seconds), checks the
// document out to a past version to replay it, and -- since a viewer can answer an
// input -- commits locally and posts the resulting update bytes to `/apply`.
//
// Kept to the surface actually used, and to the shapes verified against
// loro-crdt 1.13.8: `exportSnapshot`/`exportFrom` no longer exist (use
// `export({mode})`), `subscribeLocalUpdates` is plural in JS, and a `PeerID` is a
// decimal STRING because a u64 does not fit a JS number.
declare module 'https://esm.sh/loro-crdt@1' {
  // A peer id as decimal digits. Never a number: peer ids are u64.
  export type PeerID = `${number}`;

  // A container address, e.g. `cid:root-panes:Map` for a root container or
  // `cid:12@4821:Text` for one created by an op.
  export type ContainerID = string;

  // One operation's address: a peer id and a counter within that peer.
  //
  // `peer` is a plain `string` rather than `PeerID` on the way IN. Peer ids reach
  // this code as `String(...)` of a map key, and loro parses the digits at the
  // boundary; requiring the template-literal type would only buy a cast at every
  // call site. Values loro hands OUT (`peerIdStr`) keep the narrower type.
  export interface OpId {
    peer: string;
    counter: number;
  }

  // A contiguous run of one peer's ops, as `exportJsonInIdSpan` takes it.
  export interface IdSpan {
    peer: string;
    counter: number;
    length: number;
  }

  // Metadata for one change (a batch of ops committed together).
  export interface ChangeMeta {
    peer: PeerID;
    counter: number;
    length: number;
    lamport: number;
    timestamp: number;
  }

  export type JsonOpID = `${number}@${PeerID}`;

  // A plain value in a JSON-encoded op. A nested container appears as the string
  // `🦜:<ContainerID>` rather than its contents.
  export type JsonValue =
    | string
    | number
    | boolean
    | null
    | JsonValue[]
    | { [key: string]: JsonValue };

  // The op payloads this app reads. Map ops carry a `key`, text/list ops a `pos`;
  // everything else (marks, moves, tree ops) is opaque here and only counted.
  export type JsonOpContent =
    | { type: 'insert'; key: string; value: JsonValue }
    | { type: 'insert'; pos: number; text: string }
    | { type: 'insert'; pos: number; value: JsonValue }
    | { type: 'delete'; key: string }
    | { type: 'delete'; pos: number; len: number }
    | { type: 'mark' | 'mark_end' | 'move' | 'set' | 'unknown' };

  export interface JsonOp {
    container: ContainerID;
    counter: number;
    content: JsonOpContent;
  }

  export interface JsonChange {
    id: JsonOpID;
    timestamp: number;
    deps: JsonOpID[];
    lamport: number;
    msg: string | null;
    ops: JsonOp[];
  }

  // What `export()` produces. Only the two modes this app uses are declared: an
  // incremental update for the normal write path, and a self-contained snapshot
  // for the 409 recovery (a snapshot depends on nothing, so the server can always
  // apply it).
  export type ExportMode = { mode: 'update'; from?: VersionVector } | { mode: 'snapshot' };

  // Opaque here: the app only ever passes one straight back to `export`.
  export interface VersionVector {
    free(): void;
  }

  // A mergeable text container. The hub declares one per terminal pane under
  // `inputs` (the shared compose draft, hub.rs apply_scope); `update` diffs the
  // current content against `text` and emits minimal insert/delete ops, so two
  // viewers typing concurrently merge instead of overwriting each other.
  export class LoroText {
    toString(): string;
    update(text: string): void;
  }

  // A root or nested map. Values are LWW per key, which is exactly the semantics
  // a single-answer input control wants.
  export interface LoroMap {
    get(key: string): unknown;
    set(key: string, value: JsonValue): void;
    delete(key: string): void;
  }

  // The document's JSON projection. `panes` is the producer-owned pane set,
  // `inputs` the viewer-writable answers, `__peers` the identity table.
  export interface DocJson {
    panes?: Record<string, unknown>;
    inputs?: Record<string, unknown>;
    __peers?: Record<string, unknown>;
  }

  export class LoroDoc {
    // ----- identity -------------------------------------------------------
    readonly peerIdStr: PeerID;
    setPeerId(peer: PeerID | number): void;
    // Local commits merge into one change while their timestamps are closer than
    // this. Zero means never merge, so one answer is one row in the edit history.
    setChangeMergeInterval(interval: number): void;

    // ----- read -----------------------------------------------------------
    import(bytes: Uint8Array): void;
    toJSON(): DocJson;
    getAllChanges(): Map<PeerID, ChangeMeta[]>;
    // The ops inside one change, as JSON. Container ids come back uncompressed,
    // so `getPathToContainer` resolves them.
    exportJsonInIdSpan(span: IdSpan): JsonChange[];
    // Where a container sits in the document right now, e.g.
    // `['panes', 'scope\x1fid', 'body']`. Undefined once the container is gone.
    getPathToContainer(id: ContainerID): (string | number)[] | undefined;
    oplogVersion(): VersionVector;

    // ----- write ----------------------------------------------------------
    getMap(name: string): LoroMap;
    commit(options?: { origin?: string; timestamp?: number; message?: string }): void;
    // Fires once per LOCAL commit with the bytes to send. An `import` never
    // triggers it, so echoing the server's broadcast back cannot loop.
    subscribeLocalUpdates(callback: (bytes: Uint8Array) => void): () => void;
    export(mode: ExportMode): Uint8Array;

    // ----- time travel ----------------------------------------------------
    // Move the document view to a past version (detached) or back to the latest.
    // A detached document is read-only unless detached editing is enabled, which
    // this app deliberately leaves off: an edit made while scrubbing would fork.
    checkout(frontiers: readonly OpId[]): void;
    checkoutToLatest(): void;
    isDetached(): boolean;
  }
}
