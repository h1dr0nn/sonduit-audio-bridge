import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Rail } from './components/AppShell/Rail';
import { TitleBar } from './components/AppShell/TitleBar';
import { SettingsProvider } from './context/SettingsContext';
import { useTheme } from './hooks/useTheme';
import { useTranslation } from './i18n';
import { AboutPage } from './pages/AboutPage';
import { ConnectionPage } from './pages/ConnectionPage';
import { SettingsPage } from './pages/SettingsPage';
import { TelemetryPage } from './pages/TelemetryPage';

function Shell() {
  const [page, setPage] = useState('connection');
  const { theme, toggleTheme } = useTheme();
  const { t } = useTranslation();

  // The native acrylic tint lives in Rust and cannot read the webview's stored
  // theme, so the frontend pushes it on mount and on every toggle.
  useEffect(() => {
    invoke('set_backdrop_theme', { dark: theme === 'dark' }).catch(() => {});
  }, [theme]);

  return (
    <div className="app-shell">
      <TitleBar />
      <div className="flex min-h-0 flex-1 gap-3 px-3 pb-3">
        <Rail current={page} onSelect={setPage} t={t} />
        <main className="scroll-area min-w-0 flex-1 pr-1">
          {page === 'connection' && <ConnectionPage />}
          {page === 'telemetry' && <TelemetryPage />}
          {page === 'settings' && <SettingsPage theme={theme} onToggleTheme={toggleTheme} />}
          {page === 'about' && <AboutPage />}
        </main>
      </div>
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
