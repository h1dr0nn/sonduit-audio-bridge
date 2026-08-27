import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { CommandPalette } from './components/AppShell/CommandPalette';
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
        {/* Scrolls for the pages that are a plain stack of cards. The editor
          * claims the height with `h-full` and scrolls inside its own columns,
          * so it never reaches this scrollbar. */}
        <main className="scroll-area min-w-0 flex-1 pr-1">
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
