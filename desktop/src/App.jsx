import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { CommandPalette } from './components/AppShell/CommandPalette';
import { showToast, ToastViewport } from './components/ui/Toast';
import { Rail } from './components/AppShell/Rail';
import { TitleBar } from './components/AppShell/TitleBar';
import { SettingsProvider } from './context/SettingsContext';
import { useBridge } from './hooks/useBridge';
import { useTheme } from './hooks/useTheme';
import { useTranslation } from './i18n';
import { AboutPage } from './pages/AboutPage';
import { ConnectionPage } from './pages/ConnectionPage';
import { EditorPage } from './pages/EditorPage';
import { SettingsPage } from './pages/SettingsPage';
import { TelemetryPage } from './pages/TelemetryPage';

const SIDEBAR_KEY = 'sonduit-sidebar-expanded';

/**
 * What to say when the session moves from one link to the other.
 *
 * Keyed by where it went. A migration the user is not told about reads as a
 * glitch -- the latency figure jumps, the transport line changes, and nothing
 * explains either; the same event announced reads as the app doing its job.
 */
const LINK_TOAST = {
  usb: {
    tone: 'success',
    titleKey: 'connection.linkSwitchedToUsb',
    bodyKey: 'connection.linkSwitchedToUsbBody',
  },
  wifi: {
    tone: 'info',
    titleKey: 'connection.linkSwitchedToWifi',
    bodyKey: 'connection.linkSwitchedToWifiBody',
  },
};

/**
 * Announce a link change, and only a change.
 *
 * Lives in the shell rather than on the connection page: the backend can move
 * the session at any moment, and a user watching the telemetry page would
 * otherwise see the numbers move with nothing to explain it.
 *
 * The first transport a session reports is not a change -- the session started
 * on it -- so it is recorded and not announced. Stopping clears the memory, so
 * starting again on the same link is likewise silent.
 */
function useLinkAnnouncements(transport, t) {
  const previous = useRef(null);

  useEffect(() => {
    const was = previous.current;
    previous.current = transport ?? null;
    if (!transport || !was || was === transport) return;

    const announcement = LINK_TOAST[transport];
    if (!announcement) return;

    showToast({
      id: 'link',
      tone: announcement.tone,
      titleKey: announcement.titleKey,
      message: t(announcement.bodyKey),
    });
  }, [transport, t]);
}

function readSidebarPreference() {
  try {
    return window.localStorage.getItem(SIDEBAR_KEY) === 'true';
  } catch {
    // Private windows and blocked site data both throw here.
    return false;
  }
}

function Shell() {
  const [page, setPage] = useState('connection');
  const [sidebarExpanded, setSidebarExpanded] = useState(readSidebarPreference);
  const [paletteOpen, setPaletteOpen] = useState(false);

  const { theme, setTheme } = useTheme();
  const { t } = useTranslation();
  const bridge = useBridge();

  useLinkAnnouncements(bridge.session.transport, t);

  // The native acrylic tint lives in Rust and cannot read the webview's stored
  // theme, so the frontend pushes it on mount and on every change.
  useEffect(() => {
    invoke('set_backdrop_theme', { dark: theme === 'dark' }).catch(() => {});
  }, [theme]);

  const toggleSidebar = useCallback(() => {
    setSidebarExpanded((previous) => {
      const next = !previous;
      try {
        window.localStorage.setItem(SIDEBAR_KEY, String(next));
      } catch {
        // Remembering the preference is a convenience, never a requirement.
      }
      return next;
    });
  }, []);

  useEffect(() => {
    const onKeyDown = (event) => {
      const accel = event.ctrlKey || event.metaKey;
      if (!accel) return;

      const key = event.key.toLowerCase();
      if (key === 'k') {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      } else if (key === 'b') {
        event.preventDefault();
        toggleSidebar();
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [toggleSidebar]);

  return (
    <div className="app-shell">
      <TitleBar
        onNavigate={setPage}
        onToggleSidebar={toggleSidebar}
        onOpenPalette={() => setPaletteOpen(true)}
        sidebarExpanded={sidebarExpanded}
        bridge={bridge}
        canRescan={bridge.available}
        t={t}
      />

      <div className="flex min-h-0 flex-1 gap-3 px-3 pb-3">
        <Rail current={page} onSelect={setPage} expanded={sidebarExpanded} t={t} />
        {/* Every page now claims the height with `h-full` and scrolls inside
          * its own containers, so this scrollbar never appears. The class
          * stays for its gutter: it is what reserves the eight pixels the
          * pages' own scroll areas hang their bars in. */}
        <main className="scroll-area min-w-0 flex-1">
          {page === 'connection' && <ConnectionPage />}
          {page === 'telemetry' && <TelemetryPage />}
          {page === 'editor' && <EditorPage />}
          {page === 'settings' && <SettingsPage theme={theme} onSetTheme={setTheme} />}
          {page === 'about' && <AboutPage />}
        </main>
      </div>

      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        onNavigate={setPage}
        onToggleSidebar={toggleSidebar}
        onSetTheme={setTheme}
        theme={theme}
        canRescan={bridge.available}
        t={t}
      />

      {/* Outside the shell's own stacking order, and portaled to <body> from
        * there, so a message can appear over a page or a dialog without either
        * having to make room for it. */}
      <ToastViewport />
    </div>
  );
}

export default function App() {
  return (
    <SettingsProvider>
      <Shell />
    </SettingsProvider>
  );
}
