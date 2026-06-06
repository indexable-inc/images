// WebTransport client.
//
// One connection per peer for the lifetime of the session. Two
// interleaved streams ride one reliable bidi stream and one shared
// QUIC datagram channel:
//
//   - tag 'J' length-prefixed JSON server events (bootstrap,
//     thread/message deltas, periodic pings).
//   - tag 'B' length-prefixed binary Loro CRDT frames.
//   - tag 'P' length-prefixed JSON ping (server keepalive).
//
//   - datagrams carry Opus audio. Inbound datagrams arrive with an
//     8-byte big-endian peer id prefix stamped by the server. The
//     audio module strips that prefix and routes the rest into the
//     decoder. Outbound datagrams are raw Opus packets; the server
//     stamps our peer id before fan-out.
//
// Reconnect is auto on close with capped exponential backoff. The
// server regenerates the self-signed cert at every boot, so we
// re-fetch `/api/wt/info` on every connect rather than caching the
// hash across reconnects.

import { fetchWtInfo } from './backend';

export type ConnectionState = 'connecting' | 'open' | 'closed';

export type JsonHandler = (event: unknown) => void;
export type LoroHandler = (bytes: Uint8Array) => void;
export type DatagramHandler = (bytes: Uint8Array) => void;
export type StateHandler = (state: ConnectionState) => void;

const TAG_JSON = 0x4a; // 'J'
const TAG_BINARY = 0x42; // 'B'
const TAG_PING = 0x50; // 'P'

export interface RoomTransport {
  state(): ConnectionState;
  onState(cb: StateHandler): () => void;
  onJson(cb: JsonHandler): () => void;
  onLoroFrame(cb: LoroHandler): () => void;
  onDatagram(cb: DatagramHandler): () => void;
  sendLoro(bytes: Uint8Array): void;
  sendDatagram(bytes: Uint8Array): void;
  close(): void;
}

interface Pending {
  buffer: Uint8Array;
  offset: number;
}

let nextNativeTransportId = 1;

export function createRoomTransport(serverId: string): RoomTransport {
  if (isTauriRuntime()) {
    console.info('room: selected transport tauri-native-webtransport');
    return createTauriNativeRoomTransport(serverId);
  }
  console.info('room: selected transport browser-webtransport');
  return createBrowserRoomTransport(serverId);
}

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

function createBrowserRoomTransport(serverId: string): RoomTransport {
  const jsonHandlers = new Set<JsonHandler>();
  const loroHandlers = new Set<LoroHandler>();
  const datagramHandlers = new Set<DatagramHandler>();
  const stateHandlers = new Set<StateHandler>();

  let stateValue: ConnectionState = 'connecting';
  let transport: WebTransport | null = null;
  let writer: WritableStreamDefaultWriter<Uint8Array> | null = null;
  let datagramWriter: WritableStreamDefaultWriter<Uint8Array> | null = null;
  let backoffMs = 500;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let aborted = false;
  let connectGeneration = 0;

  function setState(next: ConnectionState) {
    if (stateValue === next) return;
    stateValue = next;
    for (const cb of stateHandlers) cb(stateValue);
  }

  async function connect() {
    if (aborted) return;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    connectGeneration++;
    const gen = connectGeneration;
    setState('connecting');

    if (typeof WebTransport === 'undefined') {
      console.error(
        'room: WebTransport API not available in this webview ' +
          '(needs Chrome/Edge/Firefox or Safari 26.4+ / macOS 26.4+)'
      );
      scheduleReconnect();
      return;
    }

    let info: Awaited<ReturnType<typeof fetchWtInfo>>;
    try {
      info = await fetchWtInfo(serverId);
    } catch (err) {
      console.warn('room: /api/wt/info failed, will retry', err);
      scheduleReconnect();
      return;
    }
    if (gen !== connectGeneration) return;

    let wt: WebTransport;
    try {
      // Copy the hash into a fresh ArrayBuffer so the type matches
      // the constructor's `BufferSource` constraint (TypeScript's
      // SharedArrayBuffer-aware narrowing rejects a borrowed view).
      const hashBuffer = new ArrayBuffer(info.certHash.byteLength);
      new Uint8Array(hashBuffer).set(info.certHash);
      wt = new WebTransport(info.wtUrl, {
        // Hash pinning lets the browser accept a self-signed cert
        // without a CA. The spec requires the cert to be valid for
        // under 14 days, which the server honors by regenerating at
        // every boot.
        serverCertificateHashes: [{ algorithm: 'sha-256', value: hashBuffer }]
      });
    } catch (err) {
      console.warn('room: WebTransport constructor failed', err);
      scheduleReconnect();
      return;
    }

    transport = wt;

    try {
      await wt.ready;
    } catch (err) {
      console.warn('room: WebTransport ready rejected', err);
      teardown(wt);
      scheduleReconnect();
      return;
    }
    if (gen !== connectGeneration) {
      teardown(wt);
      return;
    }

    let stream: WebTransportBidirectionalStream;
    try {
      stream = await wt.createBidirectionalStream();
    } catch (err) {
      console.warn('room: failed to open sync stream', err);
      teardown(wt);
      scheduleReconnect();
      return;
    }

    writer = stream.writable.getWriter();
    datagramWriter = wt.datagrams.writable.getWriter();
    backoffMs = 500;
    setState('open');

    void pumpSyncStream(stream.readable.getReader(), gen);
    void pumpDatagrams(wt.datagrams.readable.getReader(), gen);
    void watchClose(wt, gen);
  }

  async function pumpSyncStream(
    reader: ReadableStreamDefaultReader<Uint8Array>,
    gen: number
  ) {
    const pending: Pending = { buffer: new Uint8Array(0), offset: 0 };
    try {
      while (gen === connectGeneration) {
        const { value, done } = await reader.read();
        if (done) break;
        if (!value || value.byteLength === 0) continue;
        appendPending(pending, value);
        drainPending(pending);
      }
    } catch (err) {
      console.warn('room: sync stream read error', err);
    }
  }

  async function pumpDatagrams(
    reader: ReadableStreamDefaultReader<Uint8Array>,
    gen: number
  ) {
    try {
      while (gen === connectGeneration) {
        const { value, done } = await reader.read();
        if (done) break;
        if (!value || value.byteLength === 0) continue;
        for (const cb of datagramHandlers) cb(value);
      }
    } catch (err) {
      console.warn('room: datagram reader error', err);
    }
  }

  async function watchClose(wt: WebTransport, gen: number) {
    try {
      await wt.closed;
    } catch {
      // ignore — fall through to reconnect
    }
    if (gen !== connectGeneration) return;
    teardown(wt);
    scheduleReconnect();
  }

  function appendPending(pending: Pending, chunk: Uint8Array) {
    if (pending.offset === pending.buffer.byteLength) {
      pending.buffer = chunk;
      pending.offset = 0;
      return;
    }
    const remaining = pending.buffer.byteLength - pending.offset;
    const merged = new Uint8Array(remaining + chunk.byteLength);
    merged.set(pending.buffer.subarray(pending.offset), 0);
    merged.set(chunk, remaining);
    pending.buffer = merged;
    pending.offset = 0;
  }

  function drainPending(pending: Pending) {
    while (true) {
      const available = pending.buffer.byteLength - pending.offset;
      if (available < 5) break;
      const tag = pending.buffer[pending.offset];
      const dv = new DataView(
        pending.buffer.buffer,
        pending.buffer.byteOffset + pending.offset + 1,
        4
      );
      const len = dv.getUint32(0, false);
      if (available < 5 + len) break;
      const payloadStart = pending.offset + 5;
      const payload = pending.buffer.subarray(payloadStart, payloadStart + len);
      pending.offset = payloadStart + len;
      dispatchFrame(tag, payload);
    }
    if (pending.offset > 0 && pending.offset === pending.buffer.byteLength) {
      pending.buffer = new Uint8Array(0);
      pending.offset = 0;
    }
  }

  function dispatchFrame(tag: number | undefined, payload: Uint8Array) {
    if (tag === TAG_JSON) {
      try {
        const text = new TextDecoder().decode(payload);
        const parsed = JSON.parse(text);
        for (const cb of jsonHandlers) cb(parsed);
      } catch (err) {
        console.warn('room: malformed JSON frame', err);
      }
    } else if (tag === TAG_BINARY) {
      // Copy so consumers can keep references past the next drain
      const copy = payload.slice();
      for (const cb of loroHandlers) cb(copy);
    } else if (tag === TAG_PING) {
      // server liveness, nothing to do
    }
  }

  function teardown(wt: WebTransport) {
    if (transport === wt) {
      transport = null;
      writer = null;
      datagramWriter = null;
    }
    try {
      wt.close();
    } catch {
      // already closed
    }
    setState('closed');
  }

  function scheduleReconnect() {
    if (aborted) return;
    if (reconnectTimer) return;
    setState('closed');
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      void connect();
    }, backoffMs);
    backoffMs = Math.min(backoffMs * 2, 8000);
  }

  function sendLoro(bytes: Uint8Array) {
    if (!writer) return;
    const header = new Uint8Array(5);
    header[0] = TAG_BINARY;
    new DataView(header.buffer).setUint32(1, bytes.byteLength, false);
    void writer.write(header).catch(() => {});
    if (bytes.byteLength > 0) void writer.write(bytes).catch(() => {});
  }

  function sendDatagram(bytes: Uint8Array) {
    if (!datagramWriter) return;
    void datagramWriter.write(bytes).catch(() => {});
  }

  function close() {
    aborted = true;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    if (transport) teardown(transport);
  }

  void connect();

  return {
    state: () => stateValue,
    onState(cb) {
      stateHandlers.add(cb);
      cb(stateValue);
      return () => stateHandlers.delete(cb);
    },
    onJson(cb) {
      jsonHandlers.add(cb);
      return () => jsonHandlers.delete(cb);
    },
    onLoroFrame(cb) {
      loroHandlers.add(cb);
      return () => loroHandlers.delete(cb);
    },
    onDatagram(cb) {
      datagramHandlers.add(cb);
      return () => datagramHandlers.delete(cb);
    },
    sendLoro,
    sendDatagram,
    close
  };
}

interface NativeTransportEvent {
  type: 'open' | 'frame' | 'datagram' | 'closed' | 'error';
  id: number;
  tag?: number;
  payload?: number[];
  message?: string;
}

function createTauriNativeRoomTransport(serverId: string): RoomTransport {
  const jsonHandlers = new Set<JsonHandler>();
  const loroHandlers = new Set<LoroHandler>();
  const datagramHandlers = new Set<DatagramHandler>();
  const stateHandlers = new Set<StateHandler>();

  let stateValue: ConnectionState = 'connecting';
  let backoffMs = 500;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let aborted = false;
  let connectGeneration = 0;
  let sessionId: number | null = null;
  let removeEventListener: (() => void) | null = null;

  function setState(next: ConnectionState) {
    if (stateValue === next) return;
    stateValue = next;
    for (const cb of stateHandlers) cb(stateValue);
  }

  async function connect() {
    if (aborted) return;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    connectGeneration++;
    const gen = connectGeneration;
    const id = nextNativeTransportId++;
    sessionId = id;
    setState('connecting');

    let invoke: typeof import('@tauri-apps/api/core').invoke;
    let listen: typeof import('@tauri-apps/api/event').listen;
    try {
      [{ invoke }, { listen }] = await Promise.all([
        import('@tauri-apps/api/core'),
        import('@tauri-apps/api/event')
      ]);
    } catch (err) {
      console.warn('room: failed to load Tauri transport APIs', err);
      sessionId = null;
      scheduleReconnect();
      return;
    }
    if (gen !== connectGeneration) return;

    let info: Awaited<ReturnType<typeof fetchWtInfo>>;
    try {
      info = await fetchWtInfo(serverId);
    } catch (err) {
      console.warn('room: /api/wt/info failed, will retry', err);
      sessionId = null;
      scheduleReconnect();
      return;
    }
    if (gen !== connectGeneration) return;

    try {
      removeEventListener?.();
      removeEventListener = await listen<NativeTransportEvent>(
        'room://native-transport-event',
        (event) => {
          if (event.payload.id !== id) return;
          handleNativeEvent(event.payload, gen);
        }
      );
    } catch (err) {
      console.warn('room: failed to listen for native transport events', err);
      sessionId = null;
      scheduleReconnect();
      return;
    }

    try {
      await invoke('native_transport_connect', {
        id,
        wtUrl: info.wtUrl,
        certSha256Hex: bytesToHex(info.certHash)
      });
    } catch (err) {
      console.warn('room: native WebTransport connect failed', err);
      cleanupNativeListener();
      scheduleReconnect();
    }
  }

  function handleNativeEvent(event: NativeTransportEvent, gen: number) {
    if (gen !== connectGeneration) return;
    if (event.type === 'open') {
      backoffMs = 500;
      setState('open');
    } else if (event.type === 'frame') {
      dispatchFrame(event.tag, bytesFromEventPayload(event.payload));
    } else if (event.type === 'datagram') {
      const bytes = bytesFromEventPayload(event.payload);
      if (bytes.byteLength === 0) return;
      for (const cb of datagramHandlers) cb(bytes);
    } else if (event.type === 'error') {
      console.warn('room: native WebTransport error', event.message);
    } else if (event.type === 'closed') {
      cleanupNativeListener();
      if (!aborted) scheduleReconnect();
    }
  }

  function dispatchFrame(tag: number | undefined, payload: Uint8Array) {
    if (tag === TAG_JSON) {
      try {
        const text = new TextDecoder().decode(payload);
        const parsed = JSON.parse(text);
        for (const cb of jsonHandlers) cb(parsed);
      } catch (err) {
        console.warn('room: malformed JSON frame', err);
      }
    } else if (tag === TAG_BINARY) {
      const copy = payload.slice();
      for (const cb of loroHandlers) cb(copy);
    } else if (tag === TAG_PING) {
      // server liveness, nothing to do
    }
  }

  function cleanupNativeListener() {
    removeEventListener?.();
    removeEventListener = null;
    sessionId = null;
    setState('closed');
  }

  function scheduleReconnect() {
    if (aborted) return;
    if (reconnectTimer) return;
    setState('closed');
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      void connect();
    }, backoffMs);
    backoffMs = Math.min(backoffMs * 2, 8000);
  }

  function sendLoro(bytes: Uint8Array) {
    if (sessionId === null) return;
    void invokeNative('native_transport_send_loro', {
      id: sessionId,
      bytes: [...bytes]
    });
  }

  function sendDatagram(bytes: Uint8Array) {
    if (sessionId === null) return;
    void invokeNative('native_transport_send_datagram', {
      id: sessionId,
      bytes: [...bytes]
    });
  }

  function close() {
    aborted = true;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    const id = sessionId;
    cleanupNativeListener();
    if (id !== null) {
      void invokeNative('native_transport_close', { id });
    }
  }

  void connect();

  return {
    state: () => stateValue,
    onState(cb) {
      stateHandlers.add(cb);
      cb(stateValue);
      return () => stateHandlers.delete(cb);
    },
    onJson(cb) {
      jsonHandlers.add(cb);
      return () => jsonHandlers.delete(cb);
    },
    onLoroFrame(cb) {
      loroHandlers.add(cb);
      return () => loroHandlers.delete(cb);
    },
    onDatagram(cb) {
      datagramHandlers.add(cb);
      return () => datagramHandlers.delete(cb);
    },
    sendLoro,
    sendDatagram,
    close
  };
}

async function invokeNative(command: string, args: Record<string, unknown>): Promise<void> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke(command, args);
  } catch (err) {
    console.warn(`room: native transport command ${command} failed`, err);
  }
}

function bytesFromEventPayload(payload: number[] | undefined): Uint8Array {
  return new Uint8Array(payload ?? []);
}

function bytesToHex(bytes: Uint8Array): string {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}
