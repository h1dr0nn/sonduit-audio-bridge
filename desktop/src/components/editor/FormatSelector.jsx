import React from 'react';
import { cn } from '../../utils/cn';
import { useTranslation } from '../../i18n';

export function FormatSelector({ formats = [], selected, onSelect }) {
  const { t } = useTranslation();

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <div>
          <p className="text-xs uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">{t('outputLabel')}</p>
          <h3 className="text-lg font-semibold text-slate-900 dark:text-slate-100">{t('chooseFormat')}</h3>
        </div>
        <span className="rounded-full bg-white/50 px-3 py-1 text-[11px] font-semibold text-slate-700 shadow-inner dark:bg-white/10 dark:text-slate-200">
          {selected ? `${selected} ${t('selected')}` : t('preview')}
        </span>
      </div>

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
        {formats.map((format) => {
          const active = selected === format;
          return (
            <button
              type="button"
              key={format}
              onClick={() => onSelect?.(format)}
              className={cn(
                'group relative overflow-hidden rounded-2xl border px-4 py-3 text-sm font-semibold shadow-sm transition duration-smooth focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
                active
                  ? 'border-accent/60 bg-accent/10 text-accent hover:bg-accent/15'
                  : 'border-white/60 bg-white/60 text-slate-800 hover:border-white dark:border-white/10 dark:bg-white/10 dark:text-slate-50',
              )}
            >
              <span className="relative z-10">{format}</span>
              <span
                className={cn(
                  'absolute inset-0 bg-gradient-to-br from-white/60 to-white/20 opacity-0 transition duration-smooth group-hover:opacity-100 dark:from-white/10 dark:to-white/0',
                  active && 'opacity-100',
                )}
              />
            </button>
          );
        })}
      </div>
    </div>
  );
}
