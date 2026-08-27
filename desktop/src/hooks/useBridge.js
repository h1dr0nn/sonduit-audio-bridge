/**
 * Single source of truth for bridge state in the UI.
 *
 * The audio path does not exist yet: `sonduit-core` is scaffolded but the
 * desktop shell has no commands to drive it and no telemetry event stream to
 * subscribe to. Rather than scatter fake numbers through the pages, every
 * screen reads this hook, and this hook reports `available: false` with empty
 * readings until the core is wired in.
 *
 * Wiring it later means replacing the body with a Tauri event subscription;
 * the shape returned here is the shape the event will carry. Tracked as the
 * first desktop milestone in docs/roadmap.md.
 */

export const EMPTY_TELEMETRY = {
  latencyMs: null,
  bufferDepthMs: null,
  packetLossPct: null,
  driftPpm: null,
  jitterMs: null,
  latePackets: null,
  reorderedPackets: null,
  uptimeSeconds: null,
};

export const EMPTY_SESSION = {
  endpoint: null,
  sampleRate: null,
  channels: null,
  bitDepth: null,
  transport: null,
};

export function useBridge() {
  return {
    available: false,
    status: 'disconnected',
    devices: [],
    session: EMPTY_SESSION,
    telemetry: EMPTY_TELEMETRY,
  };
}
