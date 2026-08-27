import React, { useEffect, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';

/**
 * Resolve the Tauri window handle, or null when the page is being served
 * outside Tauri (vite preview, a plain browser). Every caller below no-ops in
 * that case so the shell still renders for visual work.
 */
function tauriWindow() {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

/**
 * Custom window chrome. The config sets `decorations: false`, so minimise,
 * maximise and close are ours to draw and ours to wire.
 */
export function TitleBar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const appWindow = tauriWindow();
    if (!appWindow) return undefined;

    let cancelled = false;
    const sync = () => {
      appWindow
        .isMaximized()
        .then((value) => {
          if (!cancelled) setMaximized(value);
        })
        .catch(() => {});
    };

    sync();
    const unlisten = appWindow.onResized(sync);

    return () => {
      cancelled = true;
      unlisten.then((stop) => stop()).catch(() => {});
    };
  }, []);

  const act = (fn) => () => {
    const appWindow = tauriWindow();
    if (appWindow) fn(appWindow);
  };

  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-brand">Sonduit</div>
      <div className="titlebar-drag" data-tauri-drag-region />
      <div className="titlebar-controls">
        <button
          type="button"
          className="titlebar-btn"
          onClick={act((w) => w.minimize())}
          aria-label="Minimize"
        >
          <svg width="10" height="1" viewBox="0 0 10 1" aria-hidden="true">
            <rect width="10" height="1" fill="currentColor" />
          </svg>
        </button>
        <button
          type="button"
          className="titlebar-btn"
          onClick={act((w) => w.toggleMaximize())}
          aria-label={maximized ? 'Restore' : 'Maximize'}
        >
          {maximized ? (
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
              <path
                d="M2.5 0v2.5H0V10h7.5V7.5H10V0H2.5zm0 9H1V3.5h6.5V5h-5v4zM9 6.5H3.5V1H9v5.5z"
                fill="currentColor"
              />
            </svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
              <path d="M0 0v10h10V0H0zm1 1h8v8H1V1z" fill="currentColor" />
            </svg>
          )}
        </button>
        <button
          type="button"
          className="titlebar-btn titlebar-btn--close"
          onClick={act((w) => w.close())}
          aria-label="Close"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <path
              d="M1 0L0 1l4 4-4 4 1 1 4-4 4 4 1-1-4-4 4-4-1-1-4 4L1 0z"
              fill="currentColor"
            />
          </svg>
        </button>
      </div>
    </div>
  );
}
