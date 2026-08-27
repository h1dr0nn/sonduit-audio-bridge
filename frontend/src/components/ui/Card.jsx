import React from 'react';
import { cn } from '../../utils/cn';

export function Card({ title, subtitle, actions, children, className, tone = 'default' }) {
  const toneClass = {
    default: 'card',
    invert: 'rounded-card border border-line-soft bg-invert text-ink-invert shadow-card',
    accent: 'rounded-card border border-transparent text-white shadow-card',
  }[tone];

  return (
    <section
      className={cn(toneClass, 'flex flex-col p-5', className)}
      style={tone === 'accent' ? { background: 'var(--accent-color)' } : undefined}
    >
      {(title || actions) && (
        <header className="mb-4 flex items-start justify-between gap-4">
          <div className="min-w-0">
            {title && (
              <h2
                className={cn(
                  'truncate text-lg font-semibold',
                  tone === 'default' ? 'text-ink' : 'text-current',
                )}
              >
                {title}
              </h2>
            )}
            {subtitle && (
              <p
                className={cn(
                  'mt-0.5 text-sm',
                  tone === 'default' ? 'text-ink-soft' : 'text-current opacity-70',
                )}
              >
                {subtitle}
              </p>
            )}
          </div>
          {actions && <div className="flex flex-none items-center gap-2">{actions}</div>}
        </header>
      )}
      {children}
    </section>
  );
}
