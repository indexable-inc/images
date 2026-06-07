/// The live monitor feed runs over a WebSocket (see `monitor-transport.ts`).
/// This module keeps the `openMonitorEvents` entry point stable for the app
/// shell, which is agnostic to the transport underneath.
export { openMonitorEvents } from '$lib/monitor-transport';
