import React from 'react';
import { cn } from '../../utils/cn';

/**
 * Single telemetry readout.
 *
 * `value` is null whenever the bridge is not streaming. That renders as an em
 * dash on purpose: a zero would read as a measurement, and there is nothing to
 * measure until the core is running.
 */
export function StatTile({ label, value, unit, tone = 'default', className }) {
  const hasValue = value !== null && value !== undefined;

  return (
    <div className={cn('card-sunken px-4 py-3', className)}>
      <p className="text-xs font-medium uppercase tracking-wide text-ink-faint">{label}</p>
      <p
        className={cn(
          'mt-1.5 font-mono text-2xl font-semibold tabular-nums',
          hasValue ? 'text-ink' : 'text-ink-faint',
          tone === 'warn' && hasValue && 'text-warn',
          tone === 'danger' && hasValue && 'text-danger',
        )}
      >
        {hasValue ? value : '—'}
        {hasValue && unit && (
          <span className="ml-1 text-sm font-normal text-ink-soft">{unit}</span>
        )}
      </p>
    </div>
  );
}
