import React from 'react';
import { cn } from '../../utils/cn';

const tones = {
  info: 'from-white/80 to-white/50 text-slate-900 dark:from-white/10 dark:to-white/5 dark:text-slate-50',
  success: 'from-emerald-500/20 to-emerald-400/10 text-emerald-800 dark:from-emerald-400/20 dark:to-emerald-300/10 dark:text-emerald-50',
  warning: 'from-amber-400/20 to-amber-300/10 text-amber-800 dark:from-amber-300/20 dark:to-amber-200/10 dark:text-amber-50',
};

export function ToastMessage({ title, message, tone = 'info' }) {
  return (
    <div
      className={cn(
        'relative overflow-hidden rounded-2xl border px-4 py-3 shadow-lg ring-1 ring-white/30 backdrop-blur-[28px] transition duration-smooth',
        'dark:ring-white/10',
        'bg-gradient-to-br',
        tones[tone] || tones.info,
      )}
    >
      <div className="flex items-center gap-3">
        <span className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-2xl bg-white/70 text-sm font-semibold text-accent shadow-inner dark:bg-white/10 dark:text-white">
          ⓘ
        </span>
        <div className="space-y-1">
          <p className="text-sm font-semibold leading-5">{title}</p>
          <p className="text-xs text-slate-600 dark:text-slate-300">{message}</p>
        </div>
      </div>
    </div>
  );
}
