import React from 'react';
import { FiAlertCircle, FiAlertTriangle, FiCheckCircle, FiInfo } from 'react-icons/fi';
import { cn } from '../../utils/cn';

/**
 * Toast appearance by tone.
 *
 * `error` was missing while five call sites in the editor were sending it, so
 * every failure the user could hit rendered in the neutral info style with an
 * information icon. A message that says something went wrong and looks like a
 * notice is a message people scroll past.
 *
 * The colours are the semantic tokens from index.css rather than Tailwind
 * palette names, so a change to the theme reaches these too.
 */
const TONES = {
  info: { icon: FiInfo, text: 'text-ink', accent: 'text-ink-soft' },
  success: { icon: FiCheckCircle, text: 'text-ink', accent: 'text-ok' },
  warning: { icon: FiAlertTriangle, text: 'text-ink', accent: 'text-warn' },
  error: { icon: FiAlertCircle, text: 'text-ink', accent: 'text-danger' },
};

export function ToastMessage({ title, message, tone = 'info' }) {
  const { icon: Icon, text, accent } = TONES[tone] ?? TONES.info;

  return (
    <div className={cn('card-sunken flex items-start gap-3 px-4 py-3', text)}>
      <Icon className={cn('mt-0.5 h-5 w-5 shrink-0', accent)} strokeWidth={2} />
      <div className="min-w-0">
        {title && <p className="text-sm font-semibold leading-5">{title}</p>}
        <p className="text-xs leading-relaxed text-ink-soft">{message}</p>
      </div>
    </div>
  );
}
