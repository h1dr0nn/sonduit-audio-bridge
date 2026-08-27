import React from 'react';

export function SettingRow({ label, description, children }) {
  return (
    <div className="flex items-center justify-between gap-6 border-b border-line-soft py-3.5 last:border-b-0">
      <div className="min-w-0">
        <p className="text-sm font-medium text-ink">{label}</p>
        {description && <p className="mt-0.5 text-xs text-ink-soft">{description}</p>}
      </div>
      <div className="flex flex-none items-center gap-2">{children}</div>
    </div>
  );
}
