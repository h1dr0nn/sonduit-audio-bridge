import React, { useState } from 'react';
import { ConnectionPage } from './pages/ConnectionPage';
import { SettingsPage } from './pages/SettingsPage';
import { SettingsProvider } from './context/SettingsContext';
import { useTheme } from './hooks/useTheme';

const APP_VERSION = __APP_VERSION__;

export default function App() {
  const [currentPage, setCurrentPage] = useState('connection');
  const { theme, toggleTheme } = useTheme();

  return (
    <SettingsProvider>
      {currentPage === 'connection' ? (
        <ConnectionPage onOpenSettings={() => setCurrentPage('settings')} />
      ) : (
        <SettingsPage
          onBack={() => setCurrentPage('connection')}
          theme={theme}
          onToggleTheme={toggleTheme}
          version={APP_VERSION}
        />
      )}
    </SettingsProvider>
  );
}
