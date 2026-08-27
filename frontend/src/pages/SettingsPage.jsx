import React from 'react';
import { FiArrowLeft } from 'react-icons/fi';
import { Card } from '../components/Card';
import { useSettingsContext } from '../context/SettingsContext';
import { useTranslation, LANGUAGES } from '../i18n';
import { cn } from '../utils/cn';
import { themeClasses } from '../utils/themeColors';

const ACCENT_COLORS = ['#007AFF', '#5856D6', '#34C759', '#FF9500', '#FF3B30', '#AF52DE'];
const FONT_SIZES = ['small', 'medium', 'large'];

function Row({ label, description, children }) {
  return (
    <div className="flex items-center justify-between gap-6 py-3">
      <div>
        <p className="text-sm font-medium text-slate-900 dark:text-slate-100">{label}</p>
        {description && (
          <p className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">{description}</p>
        )}
      </div>
      {children}
    </div>
  );
}

export function SettingsPage({ onBack, theme, onToggleTheme, version }) {
  const { settings, updateSetting, resetSettings } = useSettingsContext();
  const { t } = useTranslation();

  const selectClass = cn(
    'rounded-card border px-3 py-2 text-sm outline-none',
    'text-slate-900 dark:text-slate-100 dark:[color-scheme:dark]',
    themeClasses.button,
  );

  return (
    <div className="mx-auto flex min-h-screen w-full max-w-3xl flex-col gap-6 p-8">
      <header className="flex items-center gap-3">
        <button
          type="button"
          onClick={onBack}
          aria-label={t('common.back')}
          className={cn(
            'rounded-full border p-2.5 transition-colors duration-smooth',
            themeClasses.button,
          )}
        >
          <FiArrowLeft className="h-5 w-5 text-slate-700 dark:text-slate-200" />
        </button>
        <h1 className="text-2xl font-semibold text-slate-900 dark:text-slate-50">
          {t('settings.title')}
        </h1>
      </header>

      <Card title={t('settings.appearance')}>
        <Row label={t('settings.theme')} description={t('settings.themeDesc')}>
          <button
            type="button"
            onClick={onToggleTheme}
            className={cn(
              'rounded-full border px-4 py-2 text-sm font-medium transition-colors duration-smooth',
              themeClasses.button,
            )}
          >
            {theme === 'dark' ? t('common.dark') : t('common.light')}
          </button>
        </Row>

        <Row label={t('settings.accentColor')} description={t('settings.accentColorDesc')}>
          <div className="flex gap-2">
            {ACCENT_COLORS.map((color) => (
              <button
                key={color}
                type="button"
                aria-label={color}
                onClick={() => updateSetting('accentColor', color)}
                style={{ backgroundColor: color }}
                className={cn(
                  'h-7 w-7 rounded-full transition-transform duration-smooth',
                  settings.accentColor === color
                    ? 'ring-2 ring-slate-900 ring-offset-2 dark:ring-white dark:ring-offset-slate-900'
                    : 'hover:scale-110',
                )}
              />
            ))}
          </div>
        </Row>

        <Row label={t('settings.fontSize')} description={t('settings.fontSizeDesc')}>
          <select
            value={settings.fontSize}
            onChange={(event) => updateSetting('fontSize', event.target.value)}
            className={selectClass}
          >
            {FONT_SIZES.map((size) => (
              <option key={size} value={size}>
                {t(`common.${size}`)}
              </option>
            ))}
          </select>
        </Row>

        <Row label={t('settings.language')} description={t('settings.languageDesc')}>
          <select
            value={settings.language}
            onChange={(event) => updateSetting('language', event.target.value)}
            className={selectClass}
          >
            {LANGUAGES.map((language) => (
              <option key={language.code} value={language.code}>
                {language.label}
              </option>
            ))}
          </select>
        </Row>
      </Card>

      <Card title={t('settings.audio')}>
        <Row label={t('settings.targetBuffer')} description={t('settings.targetBufferDesc')}>
          <input
            type="number"
            min={5}
            max={200}
            step={5}
            value={settings.targetBufferMs}
            onChange={(event) => updateSetting('targetBufferMs', Number(event.target.value))}
            className={cn(selectClass, 'w-24 text-right tabular-nums')}
          />
        </Row>
      </Card>

      <Card title={t('settings.about')}>
        <Row label={t('settings.version')}>
          <span className="text-sm tabular-nums text-slate-600 dark:text-slate-300">
            {version ?? t('common.unknown')}
          </span>
        </Row>
        <Row label={t('settings.reset')} description={t('settings.resetDesc')}>
          <button
            type="button"
            onClick={resetSettings}
            className={cn(
              'rounded-full border px-4 py-2 text-sm font-medium transition-colors duration-smooth',
              themeClasses.button,
            )}
          >
            {t('settings.reset')}
          </button>
        </Row>
      </Card>
    </div>
  );
}
