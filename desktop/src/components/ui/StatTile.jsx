import React from 'react';
import { cn } from '../../utils/cn';

/**
 * Round a reading to the precision the eye can use.
 *
 * A raw double reached the screen as `1557.430087`, which overflowed its tile
 * and implied a precision the measurement does not have. Milliseconds are read
 * whole; a percentage below one still needs its decimals, because 0.4% and
 * 0.0% mean different things.
 */
function format(value, unit) {
  if (typeof value !== 'number') return value;
  if (!Number.isFinite(value)) return '—';

  if (unit === '%') {
    return value >= 10 ? value.toFixed(0) : value.toFixed(2);
  }
  if (unit === 'ppm') {
    return value.toFixed(1);
  }
  return Math.abs(value) >= 10 ? value.toFixed(0) : value.toFixed(1);
}

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
    <div className={cn('card-sunken min-w-0 px-4 py-3', className)}>
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
        {hasValue ? format(value, unit) : '—'}
        {hasValue && unit && <span className="ml-1 text-sm font-normal text-ink-soft">{unit}</span>}
      </p>
    </div>
  );
}
