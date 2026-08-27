import React from 'react';
import { FiSettings } from 'react-icons/fi';
import { Card } from '../components/Card';
import { StatTile } from '../components/StatTile';
import { StatusPill } from '../components/StatusPill';
import { useTranslation } from '../i18n';
import { cn } from '../utils/cn';
import { themeClasses } from '../utils/themeColors';

/**
 * The bridge core is not wired into the desktop shell yet. Until it is, the
 * panels render their empty state instead of fabricated values. Tracked in
 * docs/roadmap.md.
 */
const BRIDGE_WIRED = false;

export function ConnectionPage({ onOpenSettings }) {
  const { t } = useTranslation();

  const devices = [];
  const telemetry = {
    latencyMs: null,
    bufferDepthMs: null,
    packetLossPct: null,
    driftPpm: null,
  };

  return (
    <div className="mx-auto flex min-h-screen w-full max-w-5xl flex-col gap-6 p-8">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-semibold text-slate-900 dark:text-slate-50">Sonduit</h1>
          <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            {t('connection.subtitle')}
          </p>
        </div>
        <div className="flex items-center gap-3">
          <StatusPill state="disconnected" label={t('status.disconnected')} />
          <button
            type="button"
            onClick={onOpenSettings}
            aria-label={t('nav.settings')}
            className={cn(
              'rounded-full border p-2.5 transition-colors duration-smooth',
              themeClasses.button,
            )}
          >
            <FiSettings className="h-5 w-5 text-slate-700 dark:text-slate-200" />
          </button>
        </div>
      </header>

      <Card
        title={t('connection.devices')}
        actions={
          <button
            type="button"
            disabled={!BRIDGE_WIRED}
            className={cn(
              'rounded-full border px-4 py-2 text-sm font-medium transition-colors duration-smooth',
              'disabled:cursor-not-allowed disabled:opacity-50',
              themeClasses.button,
            )}
          >
            {t('connection.scan')}
          </button>
        }
      >
        {devices.length === 0 ? (
          <p className="py-8 text-center text-sm text-slate-500 dark:text-slate-400">
            {t('connection.noDevices')}
          </p>
        ) : null}
      </Card>

      <Card title={t('telemetry.title')}>
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <StatTile label={t('telemetry.latency')} value={telemetry.latencyMs} unit="ms" />
          <StatTile label={t('telemetry.bufferDepth')} value={telemetry.bufferDepthMs} unit="ms" />
          <StatTile label={t('telemetry.packetLoss')} value={telemetry.packetLossPct} unit="%" />
          <StatTile label={t('telemetry.drift')} value={telemetry.driftPpm} unit="ppm" />
        </div>
        {!BRIDGE_WIRED && (
          <p className="mt-4 text-center text-sm text-slate-500 dark:text-slate-400">
            {t('telemetry.noData')}
          </p>
        )}
      </Card>
    </div>
  );
}
