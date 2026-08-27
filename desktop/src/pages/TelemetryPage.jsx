import React from 'react';
import { FiActivity } from 'react-icons/fi';
import { Card } from '../components/ui/Card';
import { StatTile } from '../components/ui/StatTile';
import { useBridge } from '../hooks/useBridge';
import { useTranslation } from '../i18n';

function formatUptime(seconds) {
  if (seconds === null || seconds === undefined) return null;
  const mm = String(Math.floor(seconds / 60)).padStart(2, '0');
  const ss = String(Math.floor(seconds % 60)).padStart(2, '0');
  return `${mm}:${ss}`;
}

export function TelemetryPage() {
  const { t } = useTranslation();
  const { available, telemetry, session } = useBridge();

  return (
    <div className="flex flex-col gap-4">
      <header className="px-1">
        <h1 className="text-3xl font-semibold tracking-tight text-ink">{t('telemetry.title')}</h1>
        <p className="mt-1 text-sm text-ink-soft">{t('telemetry.subtitle')}</p>
      </header>

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <StatTile label={t('telemetry.latency')} value={telemetry.latencyMs} unit="ms" />
        <StatTile label={t('telemetry.bufferDepth')} value={telemetry.bufferDepthMs} unit="ms" />
        <StatTile label={t('telemetry.jitter')} value={telemetry.jitterMs} unit="ms" />
        <StatTile label={t('telemetry.drift')} value={telemetry.driftPpm} unit="ppm" />
        <StatTile label={t('telemetry.packetLoss')} value={telemetry.packetLossPct} unit="%" />
        <StatTile label={t('telemetry.late')} value={telemetry.latePackets} />
        <StatTile label={t('telemetry.reordered')} value={telemetry.reorderedPackets} />
        <StatTile label={t('telemetry.uptime')} value={formatUptime(telemetry.uptimeSeconds)} />
      </div>

      <Card title={t('telemetry.format')}>
        {available ? (
          <dl className="grid grid-cols-3 gap-3">
            <div className="card-sunken px-4 py-3">
              <dt className="text-xs uppercase tracking-wide text-ink-faint">
                {t('connection.sampleRate')}
              </dt>
              <dd className="mt-1 font-mono text-lg text-ink">{session.sampleRate}</dd>
            </div>
            <div className="card-sunken px-4 py-3">
              <dt className="text-xs uppercase tracking-wide text-ink-faint">
                {t('connection.channels')}
              </dt>
              <dd className="mt-1 font-mono text-lg text-ink">{session.channels}</dd>
            </div>
            <div className="card-sunken px-4 py-3">
              <dt className="text-xs uppercase tracking-wide text-ink-faint">
                {t('connection.bitDepth')}
              </dt>
              <dd className="mt-1 font-mono text-lg text-ink">{session.bitDepth}</dd>
            </div>
          </dl>
        ) : (
          <div className="flex flex-col items-center gap-2 py-10 text-center">
            <FiActivity className="h-7 w-7 text-ink-faint" strokeWidth={1.5} />
            <p className="text-sm text-ink-soft">{t('telemetry.noData')}</p>
            <p className="text-xs text-ink-faint">{t('common.notWired')}</p>
          </div>
        )}
      </Card>
    </div>
  );
}
