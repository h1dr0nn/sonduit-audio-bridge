import React from 'react';
import { cn } from '../utils/cn';
import { themeClasses } from '../utils/themeColors';

export function Card({ title, description, actions, children, className }) {
  return (
    <section
      className={cn(
        'glass-surface rounded-card border p-5 shadow-soft',
        themeClasses.card,
        className,
      )}
    >
      {(title || actions) && (
        <header className="mb-4 flex items-start justify-between gap-4">
          <div>
            {title && (
              <h2 className="text-lg font-semibold text-slate-900 dark:text-slate-100">{title}</h2>
            )}
            {description && (
              <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">{description}</p>
            )}
          </div>
          {actions}
        </header>
      )}
      {children}
    </section>
  );
}
