import React from 'react';
import { Card } from '../components/ui/Card';
import { SettingRow } from '../components/ui/SettingRow';
import { useTranslation } from '../i18n';

const APP_VERSION = __APP_VERSION__;
const REPOSITORY = 'github.com/h1dr0nn/sonduit-audio-bridge';

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
    </div>
  );
}
