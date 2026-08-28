/**
 * Single source of truth for bridge state in the UI.
 *
 * The backend owns every number. This hook does no arithmetic: it subscribes
 * to the telemetry event, and asks for a snapshot once on mount so a window
 * opened over a running session does not render an empty shell for a quarter
 * of a second.
 *
 * Only one subscription exists no matter how many components call the hook.
 * Each page mounting its own listener would multiply the event traffic by the
 * number of pages, and a page unmounting would tear down a listener another
 * page still needed.
 */

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

const TELEMETRY_EVENT = 'sonduit://telemetry';

export const EMPTY_TELEMETRY = {
  latencyMs: null,
  bufferDepthMs: null,
  packetLossPct: null,
  driftPpm: null,
  jitterMs: null,
  latePackets: null,
  reorderedPackets: null,
  uptimeSeconds: null,
  packetsSent: null,
  audioSeconds: null,
};

export const EMPTY_SESSION = {
  endpoint: null,
  sampleRate: null,
  channels: null,
  bitDepth: null,
  target: null,
  transport: null,
  wire: null,
};

const EMPTY_SNAPSHOT = {
  available: false,
  status: 'disconnected',
  session: null,
  telemetry: EMPTY_TELEMETRY,
  error: null,
};

/**
 * Whether the Tauri backend is reachable.
 *
 * The Vite dev server serves the same bundle in a plain browser, where invoke
 * would reject on every call. Checking once keeps the console clean.
 */
function hasBackend() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Module-scoped store, so every consumer shares one subscription. */
const store = {
  snapshot: EMPTY_SNAPSHOT,
  devices: [],
  subscribers: new Set(),
  unlisten: null,
  started: false,
};

function publish() {
  store.subscribers.forEach((notify) => notify());
}

function setSnapshot(next) {
  store.snapshot = next ?? EMPTY_SNAPSHOT;
  publish();
}

async function ensureSubscribed() {
  if (store.started || !hasBackend()) return;
  store.started = true;

  try {
    setSnapshot(await invoke('bridge_snapshot'));
  } catch {
    // A snapshot that cannot be read is not fatal; the event stream is the
    // real source and the empty state is correct until one arrives.
  }

  try {
    store.unlisten = await listen(TELEMETRY_EVENT, (event) => {
      setSnapshot(event.payload);
    });
  } catch {
    store.started = false;
  }
}

export function useBridge() {
  const [, forceRender] = useState(0);

  useEffect(() => {
    const notify = () => forceRender((count) => count + 1);
    store.subscribers.add(notify);
    ensureSubscribed();

    return () => {
      store.subscribers.delete(notify);
      // The listener is deliberately not torn down here. Navigating between
      // pages unmounts consumers constantly, and re-registering on every
      // navigation drops events in the gap.
    };
  }, []);

  const scan = useCallback(async (code) => {
    if (!hasBackend()) return [];
    // The backend refuses a code that is not six digits, and every device that
    // cannot prove it knows this one is dropped before it reaches the list.
    const devices = await invoke('bridge_scan', { code });
    store.devices = devices;
    publish();
    return devices;
  }, []);

  /**
   * Ask the backend for a fresh pairing invite to render as a QR code.
   *
   * Every call replaces the previous one, so the code that was on screen a
   * moment ago stops being accepted. That is deliberate: two live codes would
   * be two ways in.
   */
  const invite = useCallback(async () => {
    if (!hasBackend()) return null;
    return invoke('bridge_invite');
  }, []);

  /**
   * Wait for the phone that scanned the invite to announce itself.
   *
   * Resolves to null when nobody scanned inside the backend's window, which is
   * not an error. A device that did announce goes into the same list a
   * broadcast scan fills, so nothing downstream needs a special case for it.
   */
  const awaitPairing = useCallback(async () => {
    if (!hasBackend()) return null;
    const device = await invoke('bridge_await_pairing');
    if (device && !store.devices.some((entry) => entry.id === device.id)) {
      store.devices = [...store.devices, device];
      publish();
    }
    return device;
  }, []);

  /**
   * Stop waiting for a scan, and retire the code that was on screen.
   *
   * The backend holds the discovery port for the whole pairing window. Without
   * this, closing the dialog and reopening it inside that window asked for a
   * port the previous wait still owned, and failed.
   */
  const cancelPairing = useCallback(async () => {
    if (!hasBackend()) return;
    await invoke('bridge_cancel_pairing');
  }, []);

  const start = useCallback(async (options = {}) => {
    const session = await invoke('bridge_start', {
      options: {
        target: options.target ?? null,
        bind: options.bind ?? null,
        screamCompatible: options.screamCompatible ?? false,
      },
    });
    // The first event is up to a quarter of a second away, and a button that
    // does nothing visible for that long reads as broken.
    setSnapshot({ ...store.snapshot, status: 'connecting', session, error: null });
    return session;
  }, []);

  const stop = useCallback(async () => {
    await invoke('bridge_stop');
    setSnapshot({ ...store.snapshot, status: 'disconnected', session: null });
  }, []);

  const { snapshot } = store;

  return {
    available: snapshot.available,
    status: snapshot.status,
    error: snapshot.error,
    devices: store.devices,
    session: snapshot.session ?? EMPTY_SESSION,
    telemetry: snapshot.telemetry ?? EMPTY_TELEMETRY,
    scan,
    invite,
    awaitPairing,
    cancelPairing,
    start,
    stop,
  };
}
