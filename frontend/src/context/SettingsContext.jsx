import React, { createContext, useContext, useEffect } from 'react';
import { useSettings } from '../hooks/useSettings';

const SettingsContext = createContext(null);

const BASE_TEXT_SIZES = {
  '--text-xs': 0.75,
  '--text-sm': 0.875,
  '--text-base': 1,
  '--text-lg': 1.125,
  '--text-xl': 1.25,
  '--text-2xl': 1.5,
  '--text-3xl': 1.875,
  '--text-4xl': 2.25,
};

const TEXT_SCALES = { small: 0.9, medium: 1, large: 1.1 };

/** Expand `#rrggbb` into the `r, g, b` triplet an rgba() token needs. */
function hexToRgbTriplet(hex) {
  const match = /^#?([0-9a-f]{6})$/i.exec(hex ?? '');
  if (!match) return null;
  const value = parseInt(match[1], 16);
  return `${(value >> 16) & 255}, ${(value >> 8) & 255}, ${value & 255}`;
}

export function SettingsProvider({ children }) {
  const settingsValue = useSettings();
  const { settings } = settingsValue;

  useEffect(() => {
    const root = document.documentElement;
    root.style.setProperty('--accent-color', settings.accentColor);

    const triplet = hexToRgbTriplet(settings.accentColor);
    if (triplet) {
      root.style.setProperty('--accent-soft', `rgba(${triplet}, 0.16)`);
    }
  }, [settings.accentColor]);

  useEffect(() => {
    const root = document.documentElement;
    const scale = TEXT_SCALES[settings.fontSize] ?? 1;
    Object.entries(BASE_TEXT_SIZES).forEach(([variable, value]) => {
      root.style.setProperty(variable, `${value * scale}rem`);
    });
  }, [settings.fontSize]);

  return <SettingsContext.Provider value={settingsValue}>{children}</SettingsContext.Provider>;
}

export function useSettingsContext() {
  const context = useContext(SettingsContext);
  if (!context) {
    throw new Error('useSettingsContext must be used within SettingsProvider');
  }
  return context;
}
