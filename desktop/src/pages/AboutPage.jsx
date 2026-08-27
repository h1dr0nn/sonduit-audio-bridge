import React from 'react';
import { Card } from '../components/ui/Card';
import { SettingRow } from '../components/ui/SettingRow';
import { useTranslation } from '../i18n';

const APP_VERSION = __APP_VERSION__;
const REPOSITORY = 'github.com/h1dr0nn/sonduit-audio-bridge';

/**
 * Third-party software that ships inside the installer.
 *
 * FFmpeg is here because the LGPL requires it: an application that distributes
 * an LGPL binary has to say so and has to carry the licence text. Sonduit
 * spawns ffmpeg as a separate process and never links against it, which is
 * what keeps this project MIT, but the notice obligation applies either way.
 * See docs/licensing.md section 2.2.
 */
const BUNDLED = [
  {
    name: 'FFmpeg',
    licence: 'LGPL 3.0',
    detail: 'ffmpeg.exe, unmodified, from the BtbN LGPL build',
    source: 'github.com/BtbN/FFmpeg-Builds',
  },
];

export function AboutPage() {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-4">
      <header className="px-1">
        <h1 className="text-3xl font-semibold tracking-tight text-ink">{t('about.title')}</h1>
        <p className="mt-1 text-sm text-ink-soft">{t('about.tagline')}</p>
      </header>

      <Card>
        <SettingRow label={t('settings.version')}>
          <span className="font-mono text-sm tabular-nums text-ink-soft">{APP_VERSION}</span>
        </SettingRow>
        <SettingRow label={t('about.license')}>
          <span className="text-sm text-ink-soft">MIT</span>
        </SettingRow>
        <SettingRow label={t('about.repository')}>
          <span className="font-mono text-xs text-ink-soft">{REPOSITORY}</span>
        </SettingRow>
      </Card>

      <Card title={t('about.bundled')} subtitle={t('about.bundledNote')}>
        <ul className="flex flex-col gap-2">
          {BUNDLED.map((item) => (
            <li key={item.name} className="card-sunken px-4 py-3">
              <div className="flex items-baseline justify-between gap-3">
                <span className="text-sm font-medium text-ink">{item.name}</span>
                <span className="font-mono text-xs text-ink-soft">{item.licence}</span>
              </div>
              <p className="mt-1 text-xs text-ink-soft">{item.detail}</p>
              <p className="mt-0.5 font-mono text-xs text-ink-faint">{item.source}</p>
            </li>
          ))}
        </ul>
        <p className="mt-3 text-xs text-ink-faint">{t('about.licenceLocation')}</p>
      </Card>
    </div>
  );
}
