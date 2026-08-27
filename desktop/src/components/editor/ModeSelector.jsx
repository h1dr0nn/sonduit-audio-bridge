import React from 'react';
import { FiRefreshCw, FiZap, FiScissors, FiSliders } from 'react-icons/fi';
import { cn } from '../../utils/cn';
import { useTranslation } from '../../i18n';

export function ModeSelector({ selected, onChange }) {
  const { t } = useTranslation();
  const modes = ['format', 'enhance', 'clean', 'modify'];

  const MODE_CONFIG = {
    format: {
      icon: FiRefreshCw,
      label: t('modeConvert'),
      description: t('modeFormatDesc')
    },
    enhance: {
      icon: FiZap,
      label: t('modeMaster'),
      description: t('modeEnhanceDesc')
    },
    clean: {
      icon: FiScissors,
      label: t('modeTrim'),
      description: t('modeCleanDesc')
    },
    modify: {
      icon: FiSliders,
      label: t('modeModify'),
      description: t('modeModifyDesc')
    }
  };

  return (
    <div className="space-y-2">
      <p className="text-xs uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">{t('mode')}</p>
      <div className="space-y-1.5">
        {modes.map(mode => {
          const config = MODE_CONFIG[mode];
          const isSelected = selected === mode;
          const Icon = config.icon;

          return (
            <button
              key={mode}
              onClick={() => onChange(mode)}
              className={cn(
                'group relative flex w-full items-center gap-3 rounded-lg border px-3 py-2 text-left transition duration-smooth',
                isSelected
                  ? 'border-accent bg-accent/10 shadow-sm dark:bg-accent/20'
                  : 'border-slate-200 bg-slate-50 hover:border-slate-300 hover:bg-slate-100 dark:border-white/10 dark:bg-white/5 dark:hover:border-white/20 dark:hover:bg-white/10'
              )}
            >
              <Icon 
                className={cn(
                  'h-4 w-4 transition-colors',
                  isSelected ? 'text-accent' : 'text-slate-600 dark:text-slate-400'
                )}
              />
              <div className="flex-1 min-w-0">
                <p className={cn(
                  'text-sm font-semibold transition-colors',
                  isSelected 
                    ? 'text-accent dark:text-accent' 
                    : 'text-slate-700 dark:text-slate-200'
                )}>
                  {config.label}
                </p>
                <p className="text-xs text-slate-500 dark:text-slate-400">
                  {config.description}
                </p>
              </div>
              {isSelected && (
                <div className="flex h-4 w-4 items-center justify-center rounded-full bg-accent text-white">
                  <svg className="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={3} d="M5 13l4 4L19 7" />
                  </svg>
                </div>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}
