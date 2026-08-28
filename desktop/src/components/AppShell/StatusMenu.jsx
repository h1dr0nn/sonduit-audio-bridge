import React, { useEffect, useRef, useState } from 'react';
import { FiActivity, FiChevronDown } from 'react-icons/fi';
import { cn } from '../../utils/cn';
import { Tooltip } from '../ui/Tooltip';

const DOT = {
  disconnected: 'bg-ink-faint',
  discovering: 'bg-accent animate-live',
  connecting: 'bg-accent animate-live',
  connected: 'bg-ok animate-live',
  error: 'bg-danger',
};

/**
 * One reading. The label never wraps: a two-line label would push the value
 * out of the column it shares with every other row, and the rows only read as
 * a table while that column holds still.
 *
 * An empty string counts as absent, so a field the backend has cleared shows a
 * dash rather than a blank gap that looks like a rendering fault.
 */
function Line({ label, value }) {
  const shown = value === null || value === undefined || value === '' ? '-' : value;
  return (
    <div className="flex items-baseline justify-between gap-4 px-3 py-1.5">
      <span className="flex-none whitespace-nowrap text-xs text-ink-soft">{label}</span>
      <Tooltip label={String(shown)} side="left">
        <span className="min-w-0 truncate font-mono text-xs text-ink">
          {shown}
        </span>
      </Tooltip>
    </div>
  );
}

/**
 * Titlebar status dropdown, sitting left of the window controls. Mirrors the
 * position adb-compass gives its Binaries menu.
 *
 * It reports session state only; it has no actions, so a stale reading here
 * can never cause a wrong click.
 */
export function StatusMenu({ status, session, telemetry, available, t }) {
  const [open, setOpen] = useState(false);
  const ref = useRef(null);

  useEffect(() => {
    if (!open) return undefined;

    const onPointerDown = (event) => {
      if (!ref.current?.contains(event.target)) setOpen(false);
    };
    const dismiss = () => setOpen(false);

    document.addEventListener('mousedown', onPointerDown);
    window.addEventListener('blur', dismiss);
    window.addEventListener('resize', dismiss);

    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      window.removeEventListener('blur', dismiss);
      window.removeEventListener('resize', dismiss);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative flex h-full items-center">
      <button
        type="button"
        className="titlebar-icon-btn titlebar-icon-btn--wide"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t('status.title')}
        onClick={() => setOpen((previous) => !previous)}
      >
        <span className={cn('h-1.5 w-1.5 rounded-pill', DOT[status] ?? DOT.disconnected)} />
        <span className="text-xs font-medium">{t(`status.${status}`)}</span>
        <FiChevronDown
          className={cn('h-3 w-3 transition-transform duration-fast ease-out', open && 'rotate-180')}
          strokeWidth={2.2}
        />
      </button>

      {open && (
        <div
          role="menu"
          className={cn(
            'absolute right-0 top-full z-50 mt-1 w-96',
            'rounded-inner border border-line-soft bg-card p-1 shadow-raised',
          )}
        >
          <p className="px-3 pb-1 pt-2 text-xs font-medium uppercase tracking-wide text-ink-faint">
            {t('connection.session')}
          </p>
          <Line label={t('connection.captureSource')} value={session.endpoint} />
          <Line label={t('connection.transport')} value={session.transport} />
          <Line label={t('connection.sampleRate')} value={session.sampleRate} />
          <Line label={t('connection.channels')} value={session.channels} />

          <div className="my-1 h-px bg-line-soft" />

          <p className="px-3 pb-1 pt-1 text-xs font-medium uppercase tracking-wide text-ink-faint">
            {t('telemetry.title')}
          </p>
          <Line
            label={t('telemetry.latency')}
            value={telemetry.latencyMs === null ? null : `${telemetry.latencyMs} ms`}
          />
          <Line
            label={t('telemetry.bufferDepth')}
            value={telemetry.bufferDepthMs === null ? null : `${telemetry.bufferDepthMs} ms`}
          />
          <Line
            label={t('telemetry.packetLoss')}
            value={telemetry.packetLossPct === null ? null : `${telemetry.packetLossPct} %`}
          />

          {!available && (
            <div className="flex items-center gap-2 px-3 py-2 text-xs text-ink-faint">
              <FiActivity className="h-3.5 w-3.5 flex-none" strokeWidth={1.8} />
              {t('common.notWired')}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
