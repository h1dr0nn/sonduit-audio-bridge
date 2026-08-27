import React from 'react';
import { cn } from '../utils/cn';
import { themeClasses } from '../utils/themeColors';

/**
 * Single telemetry readout. `value` is null when the bridge is not streaming,
 * which renders as an em dash rather than a misleading zero.
 */
export function StatTile({ label, value, unit }) {
  const hasValue = value !== null && value !== undefined;
  return (
    <div className={cn('rounded-card border px-4 py-3', themeClasses.surface)}>
      <p className="text-xs uppercase tracking-wide text-slate-500 dark:text-slate-400">{label}</p>
      <p className="mt-1 text-2xl font-semibold tabular-nums text-slate-900 dark:text-slate-100">
        {hasValue ? value : '—'}
        {hasValue && unit && (
          <span className="ml-1 text-sm font-normal text-slate-500 dark:text-slate-400">{unit}</span>
        )}
      </p>
    </div>
  );
}
