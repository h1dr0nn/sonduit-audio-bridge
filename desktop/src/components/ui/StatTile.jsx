import React from 'react';
import { cn } from '../../utils/cn';
import { formatReading } from '../../utils/readings';

/**
 * Single telemetry readout.
 *
 * `value` is null whenever the far end has not reported. That renders as an em
 * dash on purpose: a zero would read as a measurement, and until a receiver
 * answers there is nothing measured to show.
 */
export function StatTile({ label, value, unit, tone = 'default', className }) {
  const hasValue = value !== null && value !== undefined;

  return (
    // Centred rather than top-aligned, because a tile that stretches to fill a
    // row would otherwise leave its reading pinned to the top with the rest of
    // the box empty below it.
    <div className={cn('card-sunken flex min-w-0 flex-col justify-center px-4 py-3', className)}>
      <p className="truncate text-xs font-medium uppercase tracking-wide text-ink-faint">
        {label}
      </p>
      <p
        className={cn(
          'mt-1.5 truncate font-mono text-2xl font-semibold tabular-nums',
          hasValue ? 'text-ink' : 'text-ink-faint',
          tone === 'warn' && hasValue && 'text-warn',
          tone === 'danger' && hasValue && 'text-danger',
        )}
      >
        {hasValue ? formatReading(value, unit) : '—'}
        {hasValue && unit && <span className="ml-1 text-sm font-normal text-ink-soft">{unit}</span>}
      </p>
    </div>
  );
}
