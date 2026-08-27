import React from 'react';
import { FiRefreshCw, FiSmartphone, FiSpeaker } from 'react-icons/fi';
import { Button } from '../components/ui/Button';
import { Card } from '../components/ui/Card';
import { StatTile } from '../components/ui/StatTile';
import { StatusPill } from '../components/ui/StatusPill';
import { useBridge } from '../hooks/useBridge';
import { useTranslation } from '../i18n';

export function ConnectionPage() {
  const { t } = useTranslation();
  const { available, status, devices, session, telemetry } = useBridge();

  return (
    <div className="flex flex-col gap-4">
      <header className="flex items-end justify-between gap-4 px-1">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight text-ink">
            {t('connection.title')}
          </h1>
          <p className="mt-1 text-sm text-ink-soft">{t('connection.subtitle')}</p>
        </div>
        <StatusPill state={status} label={t(`status.${status}`)} />
      </header>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <Card
          className="lg:col-span-2"
          title={t('connection.devices')}
          actions={
            <Button size="sm" icon={FiRefreshCw} disabled={!available}>
              {t('connection.scan')}
            </Button>
          }
        >
          {devices.length === 0 ? (
            <div className="flex flex-col items-center gap-2 py-10 text-center">
              <FiSmartphone className="h-7 w-7 text-ink-faint" strokeWidth={1.5} />
              <p className="text-sm text-ink-soft">{t('connection.noDevices')}</p>
              {!available && <p className="text-xs text-ink-faint">{t('common.notWired')}</p>}
            </div>
          ) : (
            <ul className="flex flex-col gap-2">
              {devices.map((device) => (
                <li
                  key={device.id}
                  className="card-sunken flex items-center justify-between px-4 py-3"
                >
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium text-ink">{device.name}</p>
                    <p className="truncate font-mono text-xs text-ink-soft">{device.address}</p>
                  </div>
                  <Button size="sm" variant="primary">
                    {t('connection.connect')}
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </Card>

        <Card tone="accent" title={t('connection.session')}>
          <div className="mt-auto flex flex-col gap-3">
            <div className="flex items-center gap-2 opacity-90">
              <FiSpeaker className="h-5 w-5" strokeWidth={1.75} />
              <span className="text-sm">{t('connection.endpoint')}</span>
            </div>
            <p className="font-mono text-sm opacity-80">
              {session.endpoint ?? t('connection.noEndpoint')}
            </p>
            <dl className="mt-2 grid grid-cols-3 gap-2 border-t border-white/20 pt-3 text-xs">
              <div>
                <dt className="opacity-70">{t('connection.sampleRate')}</dt>
                <dd className="mt-0.5 font-mono text-sm">{session.sampleRate ?? '—'}</dd>
              </div>
              <div>
                <dt className="opacity-70">{t('connection.channels')}</dt>
                <dd className="mt-0.5 font-mono text-sm">{session.channels ?? '—'}</dd>
              </div>
              <div>
                <dt className="opacity-70">{t('connection.bitDepth')}</dt>
                <dd className="mt-0.5 font-mono text-sm">{session.bitDepth ?? '—'}</dd>
              </div>
            </dl>
          </div>
        </Card>
      </div>

      <Card title={t('telemetry.title')} subtitle={t('telemetry.subtitle')}>
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <StatTile label={t('telemetry.latency')} value={telemetry.latencyMs} unit="ms" />
          <StatTile label={t('telemetry.bufferDepth')} value={telemetry.bufferDepthMs} unit="ms" />
          <StatTile label={t('telemetry.packetLoss')} value={telemetry.packetLossPct} unit="%" />
          <StatTile label={t('telemetry.drift')} value={telemetry.driftPpm} unit="ppm" />
        </div>
        {!available && (
          <p className="mt-4 text-center text-xs text-ink-faint">{t('telemetry.noData')}</p>
        )}
      </Card>
    </div>
  );
}
