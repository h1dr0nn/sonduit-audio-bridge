import React from 'react';
import { FiActivity } from 'react-icons/fi';
import { Card } from '../components/ui/Card';
import { StatTile } from '../components/ui/StatTile';
import { useBridge } from '../hooks/useBridge';
import { useTranslation } from '../i18n';

/* A readout that has been given more height than its two lines need, so the
 * number sits in the middle of its tile instead of clinging to the top. */
const READOUT = 'flex flex-col justify-center';

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
    /* Fills the window rather than stacking to whatever height the content
     * happens to want.
     *
     * `min-h-0` is on every flex and grid child in the chain because that is
     * what makes it work: such a child defaults to `min-height: auto`, refuses
     * to shrink below its content, and pushes the overflow out to the shell. */
    <div className="flex h-full min-h-0 flex-col gap-4">
      <header className="shrink-0 px-1">
        <h1 className="text-3xl font-semibold tracking-tight text-ink">{t('telemetry.title')}</h1>
        <p className="mt-1 text-sm text-ink-soft">{t('telemetry.subtitle')}</p>
      </header>

      {/* Eight readouts and one panel are all this page has, so the leftover
        * height is shared out between them instead of pooling at the bottom.
        * The readouts take the larger share: they are what the page is for,
        * and the format panel is three fixed values whatever the window does. */}
      <div className="grid min-h-0 flex-1 grid-rows-[minmax(0,3fr)_minmax(0,2fr)] gap-4">
        {/* Four across, two down. The window cannot be narrower than 960, so a
          * responsive fallback to two columns is unreachable in the shipped
          * application and only ever cost the grid its second row. */}
        <div className="grid min-h-0 grid-cols-4 grid-rows-2 gap-3">
          <StatTile
            className={READOUT}
            label={t('telemetry.latency')}
            value={telemetry.latencyMs}
            unit="ms"
          />
          <StatTile
            className={READOUT}
            label={t('telemetry.bufferDepth')}
            value={telemetry.bufferDepthMs}
            unit="ms"
          />
          <StatTile
            className={READOUT}
            label={t('telemetry.jitter')}
            value={telemetry.jitterMs}
            unit="ms"
          />
          <StatTile
            className={READOUT}
            label={t('telemetry.drift')}
            value={telemetry.driftPpm}
            unit="ppm"
          />
          <StatTile
            className={READOUT}
            label={t('telemetry.packetLoss')}
            value={telemetry.packetLossPct}
            unit="%"
          />
          <StatTile
            className={READOUT}
            label={t('telemetry.late')}
            value={telemetry.latePackets}
          />
          <StatTile
            className={READOUT}
            label={t('telemetry.reordered')}
            value={telemetry.reorderedPackets}
          />
          <StatTile
            className={READOUT}
            label={t('telemetry.uptime')}
            value={formatUptime(telemetry.uptimeSeconds)}
          />
        </div>

        <Card className="min-h-0" title={t('telemetry.format')}>
          {available ? (
            <dl className="grid min-h-0 flex-1 grid-cols-4 gap-3">
              <div className={`card-sunken px-4 py-3 ${READOUT}`}>
                <dt className="text-xs uppercase tracking-wide text-ink-faint">
                  {t('connection.sampleRate')}
                </dt>
                <dd className="mt-1 font-mono text-lg text-ink">{session.sampleRate}</dd>
              </div>
              <div className={`card-sunken px-4 py-3 ${READOUT}`}>
                <dt className="text-xs uppercase tracking-wide text-ink-faint">
                  {t('connection.channels')}
                </dt>
                <dd className="mt-1 font-mono text-lg text-ink">{session.channels}</dd>
              </div>
              <div className={`card-sunken px-4 py-3 ${READOUT}`}>
                <dt className="text-xs uppercase tracking-wide text-ink-faint">
                  {t('connection.bitDepth')}
                </dt>
                <dd className="mt-1 font-mono text-lg text-ink">{session.bitDepth}</dd>
              </div>
              {/* Beside the format because it is a fact about the same stream,
                * and never inferred here: it is what the send loop actually
                * did with the packets. */}
              <div className={`card-sunken px-4 py-3 ${READOUT}`}>
                <dt className="text-xs uppercase tracking-wide text-ink-faint">
                  {t('connection.encryption')}
                </dt>
                <dd className="mt-1 text-lg text-ink">
                  {session.encrypted === null || session.encrypted === undefined
                    ? '—'
                    : t(session.encrypted ? 'connection.encryptionOn' : 'connection.encryptionOff')}
                </dd>
                {telemetry.refusedReports > 0 && (
                  <p className="mt-1 text-xs text-ink-faint">
                    {t('telemetry.refusedReports')}: {telemetry.refusedReports}
                  </p>
                )}
              </div>
            </dl>
          ) : (
            <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 text-center">
              <FiActivity className="h-7 w-7 text-ink-faint" strokeWidth={1.5} />
              <p className="text-sm text-ink-soft">{t('telemetry.noData')}</p>
              <p className="text-xs text-ink-faint">{t('common.notWired')}</p>
            </div>
          )}
        </Card>
      </div>
    </div>
  );
}
