import React, { useEffect, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { FiSearch, FiSidebar } from 'react-icons/fi';
import { MenuDropdown } from './MenuDropdown';
import { StatusMenu } from './StatusMenu';
import { Tooltip } from '../ui/Tooltip';

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
 * Custom window chrome. The config sets `decorations: false`, so the window
 * controls are ours to draw and ours to wire.
 *
 * Left cluster, in order: application menu, sidebar toggle, command palette.
 * Right cluster: the session status menu, then minimise, maximise and close.
 */
export function TitleBar({
  onNavigate,
  onToggleSidebar,
  onOpenPalette,
  sidebarExpanded,
  bridge,
  canRescan,
  t,
}) {
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

  // Names the outcome of the press, not the current state.
  const maximizeLabel = maximized ? t('window.restore') : t('window.maximize');

  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-cluster">
        <MenuDropdown
          onNavigate={onNavigate}
          onOpenPalette={onOpenPalette}
          onToggleSidebar={onToggleSidebar}
          t={t}
        />

        <Tooltip label={`${t('nav.toggleSidebar')} (Ctrl+B)`}>
          <button
            type="button"
            className="titlebar-icon-btn"
            onClick={onToggleSidebar}
            aria-label={t('nav.toggleSidebar')}
            aria-pressed={sidebarExpanded}
          >
            <FiSidebar className="h-4 w-4" strokeWidth={1.9} />
          </button>
        </Tooltip>

        <Tooltip label={`${t('nav.commandPalette')} (Ctrl+K)`}>
          <button
            type="button"
            className="titlebar-icon-btn"
            onClick={onOpenPalette}
            aria-label={t('nav.commandPalette')}
          >
            <FiSearch className="h-4 w-4" strokeWidth={1.9} />
          </button>
        </Tooltip>
      </div>

      <div className="titlebar-drag" data-tauri-drag-region />

      <div className="titlebar-cluster titlebar-cluster--right">
        <StatusMenu
          status={bridge.status}
          session={bridge.session}
          telemetry={bridge.telemetry}
          available={bridge.available}
          t={t}
        />
      </div>

      <div className="titlebar-controls">
        <Tooltip label={t('window.minimize')}>
          <button
            type="button"
            className="titlebar-btn"
            onClick={act((w) => w.minimize())}
            aria-label={t('window.minimize')}
          >
            <svg width="10" height="1" viewBox="0 0 10 1" aria-hidden="true">
              <rect width="10" height="1" fill="currentColor" />
            </svg>
          </button>
        </Tooltip>
        {/* One icon in both states, by request: a control that swaps its glyph
          * while the pointer rests on it reads as the window flickering. What
          * the press will do is carried by the label instead, which is what a
          * screen reader announces in either case. */}
        <Tooltip label={maximizeLabel}>
          <button
            type="button"
            className="titlebar-btn"
            onClick={act((w) => w.toggleMaximize())}
            aria-label={maximizeLabel}
          >
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
              <path d="M0 0v10h10V0H0zm1 1h8v8H1V1z" fill="currentColor" />
            </svg>
          </button>
        </Tooltip>
        <Tooltip label={t('window.close')}>
          <button
            type="button"
            className="titlebar-btn titlebar-btn--close"
            onClick={act((w) => w.close())}
            aria-label={t('window.close')}
          >
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
              <path
                d="M1 0L0 1l4 4-4 4 1 1 4-4 4 4 1-1-4-4 4-4-1-1-4 4L1 0z"
                fill="currentColor"
              />
            </svg>
          </button>
        </Tooltip>
      </div>
    </div>
  );
}
