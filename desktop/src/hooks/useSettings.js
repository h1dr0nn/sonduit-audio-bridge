import { useState, useEffect } from 'react';
import { Store } from '@tauri-apps/plugin-store';
import { FALLBACK_LANGUAGE, detectSystemLanguage } from '../i18n/languages';

const STORE_FILENAME = 'settings.json';
const store = new Store(STORE_FILENAME);

export const DEFAULT_SETTINGS = {
  // Audio bridge
  targetBufferMs: 30,
  preferredTransport: 'auto',
  // Which Windows output endpoint to tap. Empty means the system default,
  // which is what the app did before there was a choice at all.
  captureDeviceId: '',
  // The name that went with that id when it was chosen. Kept because a device
  // that has been unplugged is no longer in the list the settings page fetches,
  // and a dropdown that goes blank looks like the setting was lost rather than
  // like the headset is out.
  captureDeviceName: '',

  // Appearance
  accentColor: '#7c93e8',
  fontSize: 'medium',
  // Placeholder only. The first run replaces it with the operating system
  // language; every run after that reads whatever the user last chose.
  language: FALLBACK_LANGUAGE,
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
          const firstRun = { ...DEFAULT_SETTINGS, language: detectSystemLanguage() };
          setSettings(firstRun);
          await store.set('settings', firstRun);
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
