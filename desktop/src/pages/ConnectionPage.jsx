import React, { useCallback, useMemo, useState } from 'react';
import { FiPlay, FiRefreshCw, FiSmartphone, FiSpeaker, FiSquare } from 'react-icons/fi';
import { Button } from '../components/ui/Button';
import { Card } from '../components/ui/Card';
import { StatTile } from '../components/ui/StatTile';
import { StatusPill } from '../components/ui/StatusPill';
import { TextField } from '../components/ui/TextField';
import { useBridge } from '../hooks/useBridge';
import { useTranslation } from '../i18n';

/**
 * A device the user has picked, or an address typed by hand.
 *
 * Kept in one piece of state rather than two so the two cannot both be set,
 * which would leave the start button unable to say what it would do.
 */
const NO_TARGET = { kind: 'multicast', value: null };

export function ConnectionPage() {
  const { t } = useTranslation();
  const { available, status, error, devices, session, telemetry, scan, start, stop } = useBridge();

  const [target, setTarget] = useState(NO_TARGET);
  const [typed, setTyped] = useState('');
  const [scanning, setScanning] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState(null);

  const running = status === 'connected' || status === 'connecting';

  const targetLabel = useMemo(() => {
    if (target.kind === 'device') return target.value;
    if (target.kind === 'manual') return typed.trim() || t('connection.multicast');
    return t('connection.multicast');
  }, [target, typed, t]);

  const handleScan = useCallback(async () => {
    setScanning(true);
    setFailure(null);
    try {
      await scan();
    } catch (reason) {
      setFailure(String(reason));
    } finally {
      setScanning(false);
    }
  }, [scan]);

  const handleStart = useCallback(async () => {
    setBusy(true);
    setFailure(null);
    try {
      const address = target.kind === 'manual' ? typed.trim() : target.value;
      await start({ target: address || null });
    } catch (reason) {
      setFailure(String(reason));
    } finally {
      setBusy(false);
    }
  }, [start, target, typed]);

  const handleStop = useCallback(async () => {
    setBusy(true);
    try {
      await stop();
    } catch (reason) {
      setFailure(String(reason));
    } finally {
      setBusy(false);
    }
  }, [stop]);

  const shown = failure ?? error;

  return (
    <div className="flex flex-col gap-4">
      <header className="flex items-end justify-between gap-4 px-1">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight text-ink">
            {t('connection.title')}
          </h1>
          <p className="mt-1 text-sm text-ink-soft">{t('connection.subtitle')}</p>
        </div>
        <div className="flex items-center gap-3">
          <StatusPill state={status} label={t(`status.${status}`)} />
          {running ? (
            <Button variant="quiet" icon={FiSquare} disabled={busy} onClick={handleStop}>
              {t('connection.stop')}
            </Button>
          ) : (
            <Button
              variant="accent"
              icon={FiPlay}
              disabled={!available || busy}
              onClick={handleStart}
            >
              {busy ? t('connection.starting') : t('connection.start')}
            </Button>
          )}
        </div>
      </header>

      {shown && (
        <div className="card-sunken border border-line-soft px-4 py-3 text-sm text-ink">
          {shown}
        </div>
      )}

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <Card
          className="lg:col-span-2"
          title={t('connection.devices')}
          actions={
            <Button
              size="sm"
              icon={FiRefreshCw}
              disabled={!available || scanning}
              onClick={handleScan}
            >
              {scanning ? t('connection.scanning') : t('connection.scan')}
            </Button>
          }
        >
          {devices.length === 0 ? (
            <div className="flex flex-col items-center gap-2 py-8 text-center">
              <FiSmartphone className="h-7 w-7 text-ink-faint" strokeWidth={1.5} />
              <p className="text-sm text-ink-soft">{t('connection.noDevices')}</p>
              {!available && <p className="text-xs text-ink-faint">{t('connection.captureOnly')}</p>}
            </div>
          ) : (
            <ul className="flex flex-col gap-2">
              {devices.map((device) => {
                const selected = target.kind === 'device' && target.value === device.address;
                return (
                  <li
                    key={device.id}
                    className="card-sunken flex items-center justify-between px-4 py-3"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-sm font-medium text-ink">{device.name}</p>
                      <p className="truncate font-mono text-xs text-ink-soft">{device.address}</p>
                    </div>
                    <Button
                      size="sm"
                      variant={selected ? 'primary' : 'quiet'}
                      disabled={running}
                      onClick={() => setTarget({ kind: 'device', value: device.address })}
                    >
                      {t('connection.connect')}
                    </Button>
                  </li>
                );
              })}
            </ul>
          )}

          <div className="mt-4 border-t border-line-soft pt-4">
            <TextField
              id="manual-address"
              label={t('connection.address')}
              placeholder={t('connection.addressHint')}
              value={typed}
              disabled={running}
              onChange={(event) => {
                setTyped(event.target.value);
                setTarget(event.target.value.trim() ? { kind: 'manual', value: null } : NO_TARGET);
              }}
            />
          </div>
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

            <div className="text-xs opacity-70">{t('connection.target')}</div>
            <p className="-mt-2 truncate font-mono text-sm opacity-80">
              {session.target ?? targetLabel}
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
          <StatTile label={t('telemetry.packetsSent')} value={telemetry.packetsSent} />
        </div>
        {!running && (
          <p className="mt-4 text-center text-xs text-ink-faint">{t('telemetry.noData')}</p>
        )}
      </Card>
    </div>
  );
}
