import React from 'react';
import { Button } from '../components/ui/Button';
import { Card } from '../components/ui/Card';
import { SettingRow } from '../components/ui/SettingRow';
import { useSettingsContext } from '../context/SettingsContext';
import { useTranslation, LANGUAGES } from '../i18n';
import { cn } from '../utils/cn';

const ACCENT_COLORS = ['#7c93e8', '#007aff', '#5856d6', '#2fa96b', '#d99b28', '#d9503f'];
const FONT_SIZES = ['small', 'medium', 'large'];
const TRANSPORTS = ['auto', 'wifi', 'usb'];

const TRANSPORT_LABEL_KEY = {
  auto: 'connection.transportAuto',
  wifi: 'connection.transportWifi',
  usb: 'connection.transportUsb',
};

const controlClass = cn(
  'h-9 rounded-pill border border-line-soft bg-sunken px-3 text-sm text-ink',
  'outline-none transition-colors duration-fast ease-out hover:border-line-strong',
);

export function SettingsPage({ theme, onToggleTheme }) {
  const { settings, updateSetting, resetSettings } = useSettingsContext();
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-4">
      <header className="px-1">
        <h1 className="text-3xl font-semibold tracking-tight text-ink">{t('settings.title')}</h1>
      </header>

      <Card title={t('settings.appearance')}>
        <SettingRow label={t('settings.theme')} description={t('settings.themeDesc')}>
          <Button onClick={onToggleTheme} size="sm">
            {theme === 'dark' ? t('common.dark') : t('common.light')}
          </Button>
        </SettingRow>

        <SettingRow label={t('settings.accentColor')} description={t('settings.accentColorDesc')}>
          {ACCENT_COLORS.map((color) => (
            <button
              key={color}
              type="button"
              aria-label={color}
              onClick={() => updateSetting('accentColor', color)}
              style={{ backgroundColor: color }}
              className={cn(
                'h-6 w-6 rounded-pill transition-transform duration-fast ease-out',
                settings.accentColor === color
                  ? 'ring-2 ring-offset-2 ring-[var(--text-primary)] ring-offset-[var(--surface-card)]'
                  : 'hover:scale-110',
              )}
            />
          ))}
        </SettingRow>

        <SettingRow label={t('settings.fontSize')} description={t('settings.fontSizeDesc')}>
          <select
            value={settings.fontSize}
            onChange={(event) => updateSetting('fontSize', event.target.value)}
            className={controlClass}
          >
            {FONT_SIZES.map((size) => (
              <option key={size} value={size}>
                {t(`common.${size}`)}
              </option>
            ))}
          </select>
        </SettingRow>

        <SettingRow label={t('settings.language')} description={t('settings.languageDesc')}>
          <select
            value={settings.language}
            onChange={(event) => updateSetting('language', event.target.value)}
            className={controlClass}
          >
            {LANGUAGES.map((language) => (
              <option key={language.code} value={language.code}>
                {language.label}
              </option>
            ))}
          </select>
        </SettingRow>
      </Card>

      <Card title={t('settings.audio')}>
        <SettingRow label={t('settings.targetBuffer')} description={t('settings.targetBufferDesc')}>
          <input
            type="number"
            min={5}
            max={200}
            step={5}
            value={settings.targetBufferMs}
            onChange={(event) => updateSetting('targetBufferMs', Number(event.target.value))}
            className={cn(controlClass, 'w-24 text-right font-mono tabular-nums')}
          />
        </SettingRow>

        <SettingRow label={t('connection.transport')}>
          <select
            value={settings.preferredTransport}
            onChange={(event) => updateSetting('preferredTransport', event.target.value)}
            className={controlClass}
          >
            {TRANSPORTS.map((transport) => (
              <option key={transport} value={transport}>
                {t(TRANSPORT_LABEL_KEY[transport])}
              </option>
            ))}
          </select>
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
