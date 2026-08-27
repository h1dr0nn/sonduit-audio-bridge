import React, { useEffect } from 'react';
import { FiAlertTriangle, FiInfo, FiX } from 'react-icons/fi';
import { Button } from './Button';
import { cn } from '../../utils/cn';
import { useTranslation } from '../../i18n';

const TONES = {
  info: { icon: FiInfo, colour: 'text-ink-soft' },
  warning: { icon: FiAlertTriangle, colour: 'text-warn' },
  danger: { icon: FiAlertTriangle, colour: 'text-danger' },
};

/**
 * The application's only dialog.
 *
 * It exists because `window.alert` was still being used in two places. A
 * native dialog is drawn by the operating system, so it ignores the theme,
 * ignores the window chrome, and blocks the whole webview thread while it is
 * up. Every one of those is visible to the user as the application briefly
 * turning into something else.
 *
 * Placement comes from `.modal-center`, which centres against the area below
 * the titlebar rather than against the window. Centring against the window
 * puts the panel visibly high, because the titlebar overlays the top of that
 * area and is not part of it.
 */
export function Dialog({
  open,
  title,
  children,
  tone = 'info',
  confirmLabel,
  onConfirm,
  cancelLabel,
  onClose,
}) {
  const { t } = useTranslation();

  useEffect(() => {
    if (!open) return undefined;
    // Escape closes. A dialog that can only be dismissed by hitting a specific
    // button is a dialog people learn to dread.
    const onKey = (event) => {
      if (event.key === 'Escape') onClose?.();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;

  const { icon: Icon, colour } = TONES[tone] ?? TONES.info;

  return (
    <>
      <div className="modal-backdrop" role="presentation" onMouseDown={onClose} />
      <div className="modal-center">
        <div
          role="dialog"
          aria-modal="true"
          aria-label={title}
          className="modal-surface w-[440px] max-w-[90vw] rounded-card p-5"
        >
          <div className="flex items-start justify-between gap-3">
            <div className="flex items-center gap-2.5">
              <Icon className={cn('h-5 w-5 shrink-0', colour)} strokeWidth={2} />
              <h2 className="text-base font-semibold text-ink">{title}</h2>
            </div>
            <button
              type="button"
              onClick={onClose}
              aria-label={t('close')}
              className="-mr-1 -mt-1 flex h-8 w-8 items-center justify-center rounded-full text-ink-faint transition-colors hover:bg-sunken hover:text-ink"
            >
              <FiX className="h-4 w-4" strokeWidth={2} />
            </button>
          </div>

          {children && <div className="mt-3 text-sm leading-relaxed text-ink-soft">{children}</div>}

          <div className="mt-5 flex justify-end gap-2">
            {cancelLabel && (
              <Button size="sm" variant="quiet" onClick={onClose}>
                {cancelLabel}
              </Button>
            )}
            <Button
              size="sm"
              variant="primary"
              onClick={() => {
                onConfirm?.();
                onClose?.();
              }}
            >
              {confirmLabel ?? t('close')}
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}
