import { useState, useEffect } from 'react';
import { Store } from '@tauri-apps/plugin-store';

const STORE_FILENAME = 'settings.json';
const store = new Store(STORE_FILENAME);

export const DEFAULT_SETTINGS = {
  // Audio bridge
  targetBufferMs: 30,
  preferredTransport: 'auto',

  // Appearance
  accentColor: '#7c93e8',
  fontSize: 'medium',
  language: 'en',
};

export function useSettings() {
  const [settings, setSettings] = useState(DEFAULT_SETTINGS);
  const [isLoaded, setIsLoaded] = useState(false);

  useEffect(() => {
    const loadSettings = async () => {
      try {
        const savedSettings = await store.get('settings');
        if (savedSettings) {
          setSettings({ ...DEFAULT_SETTINGS, ...savedSettings });
        } else {
          await store.set('settings', DEFAULT_SETTINGS);
          await store.save();
        }
      } catch (error) {
        console.error('Failed to load settings:', error);
      } finally {
        setIsLoaded(true);
      }
    };
    loadSettings();
  }, []);

  useEffect(() => {
    if (!isLoaded) return;

    const saveSettings = async () => {
      try {
        await store.set('settings', settings);
        await store.save();
      } catch (error) {
        console.error('Failed to save settings:', error);
      }
    };
    saveSettings();
  }, [settings, isLoaded]);

  const updateSetting = (key, value) => {
    setSettings((prev) => ({ ...prev, [key]: value }));
  };

  const updateSettings = (updates) => {
    setSettings((prev) => ({ ...prev, ...updates }));
  };

  const resetSettings = async () => {
    setSettings(DEFAULT_SETTINGS);
    try {
      await store.set('settings', DEFAULT_SETTINGS);
      await store.save();
    } catch (error) {
      console.error('Failed to reset settings:', error);
    }
  };

  return {
    settings,
    updateSetting,
    updateSettings,
    resetSettings,
    isLoaded,
  };
}
