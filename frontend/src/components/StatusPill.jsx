import React from 'react';
import { cn } from '../utils/cn';

const tones = {
  disconnected: 'bg-slate-400/15 text-slate-600 dark:text-slate-300',
  discovering: 'bg-sky-400/15 text-sky-700 dark:text-sky-300',
  connecting: 'bg-amber-400/20 text-amber-700 dark:text-amber-300',
  connected: 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300',
  error: 'bg-rose-500/15 text-rose-700 dark:text-rose-300',
};

export function StatusPill({ state, label }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-2 rounded-full px-3 py-1 text-sm font-medium',
        tones[state] ?? tones.disconnected,
      )}
    >
      <span className="h-2 w-2 rounded-full bg-current" />
      {label}
    </span>
  );
}
