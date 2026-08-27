import React from 'react';
import { Button } from '../components/ui/Button';
import { Card } from '../components/ui/Card';
import { NumberField } from '../components/ui/NumberField';
import { Select } from '../components/ui/Select';
import { SettingRow } from '../components/ui/SettingRow';
import { useSettingsContext } from '../context/SettingsContext';
import { useTranslation, LANGUAGES } from '../i18n';
import { cn } from '../utils/cn';

const ACCENT_COLORS = ['#7c93e8', '#007aff', '#5856d6', '#2fa96b', '#d99b28', '#d9503f'];
const FONT_SIZES = ['small', 'medium', 'large'];
const TRANSPORTS = ['auto', 'wifi', 'usb'];
const THEMES = ['light', 'dark'];

const TRANSPORT_LABEL_KEY = {
  auto: 'connection.transportAuto',
  wifi: 'connection.transportWifi',
  usb: 'connection.transportUsb',
};

export function SettingsPage({ theme, onSetTheme }) {
  const { settings, updateSetting, resetSettings } = useSettingsContext();
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-4">
      <header className="px-1">
        <h1 className="text-3xl font-semibold tracking-tight text-ink">{t('settings.title')}</h1>
      </header>

      <Card title={t('settings.appearance')}>
        <SettingRow label={t('settings.theme')} description={t('settings.themeDesc')}>
          <Select
            className="w-36"
            ariaLabel={t('settings.theme')}
            value={theme}
            onChange={onSetTheme}
            options={THEMES.map((mode) => ({ value: mode, label: t(`common.${mode}`) }))}
          />
        </SettingRow>

        <SettingRow label={t('settings.accentColor')} description={t('settings.accentColorDesc')}>
          {ACCENT_COLORS.map((color) => (
            <button
              key={color}
              type="button"
              aria-label={color}
              aria-pressed={settings.accentColor === color}
              onClick={() => updateSetting('accentColor', color)}
              style={{ backgroundColor: color }}
              className={cn(
                'h-6 w-6 rounded-pill transition-transform duration-fast ease-out',
                settings.accentColor === color
                  ? 'ring-2 ring-[var(--text-primary)] ring-offset-2 ring-offset-[var(--surface-card)]'
                  : 'hover:scale-110',
              )}
            />
          ))}
        </SettingRow>

        <SettingRow label={t('settings.fontSize')} description={t('settings.fontSizeDesc')}>
          <Select
            className="w-36"
            ariaLabel={t('settings.fontSize')}
            value={settings.fontSize}
            onChange={(next) => updateSetting('fontSize', next)}
            options={FONT_SIZES.map((size) => ({ value: size, label: t(`common.${size}`) }))}
          />
        </SettingRow>

        <SettingRow label={t('settings.language')} description={t('settings.languageDesc')}>
          <Select
            className="w-36"
            ariaLabel={t('settings.language')}
            value={settings.language}
            onChange={(next) => updateSetting('language', next)}
            options={LANGUAGES.map((language) => ({
              value: language.code,
              label: language.label,
            }))}
          />
        </SettingRow>
      </Card>

      <Card title={t('settings.audio')}>
        <SettingRow label={t('settings.targetBuffer')} description={t('settings.targetBufferDesc')}>
          <NumberField
            ariaLabel={t('settings.targetBuffer')}
            value={settings.targetBufferMs}
            onChange={(next) => updateSetting('targetBufferMs', next)}
            min={5}
            max={200}
            step={5}
            unit="ms"
          />
        </SettingRow>

        <SettingRow label={t('connection.transport')}>
          <Select
            className="w-36"
            ariaLabel={t('connection.transport')}
            value={settings.preferredTransport}
            onChange={(next) => updateSetting('preferredTransport', next)}
            options={TRANSPORTS.map((transport) => ({
              value: transport,
              label: t(TRANSPORT_LABEL_KEY[transport]),
            }))}
          />
        </SettingRow>

        <SettingRow label={t('settings.reset')} description={t('settings.resetDesc')}>
          <Button onClick={resetSettings} size="sm">
            {t('settings.reset')}
          </Button>
        </SettingRow>
      </Card>
    </div>
  );
}
