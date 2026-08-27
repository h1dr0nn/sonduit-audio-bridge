import React from 'react';
import { cn } from '../../utils/cn';

const TONES = {
  disconnected: 'bg-sunken text-ink-soft border-line-soft',
  discovering: 'bg-accent-soft text-accent border-transparent',
  connecting: 'bg-accent-soft text-accent border-transparent',
  connected: 'border-transparent',
  error: 'border-transparent',
};

const DOT = {
  disconnected: 'bg-ink-faint',
  discovering: 'bg-accent animate-live',
  connecting: 'bg-accent animate-live',
  connected: 'bg-ok animate-live',
  error: 'bg-danger',
};

export function StatusPill({ state = 'disconnected', label }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-2 rounded-pill border px-3 py-1.5 text-sm font-medium',
        TONES[state] ?? TONES.disconnected,
      )}
      style={
        state === 'connected'
          ? { background: 'rgba(47, 169, 107, 0.14)', color: 'var(--ok)' }
          : state === 'error'
            ? { background: 'rgba(217, 80, 63, 0.14)', color: 'var(--danger)' }
            : undefined
      }
    >
      <span className={cn('h-1.5 w-1.5 rounded-pill', DOT[state] ?? DOT.disconnected)} />
      {label}
    </span>
  );
}
