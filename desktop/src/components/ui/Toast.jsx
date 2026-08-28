import React, { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';
import { FiAlertCircle, FiCheckCircle, FiCopy, FiInfo, FiX } from 'react-icons/fi';
import { useTranslation } from '../../i18n';
import { cn } from '../../utils/cn';

/**
 * Transient notifications, rendered over the page rather than inside it.
 *
 * They used to be a card in the middle of the connection page, which pushed
 * everything below it down the moment anything went wrong. A message about a
 * failure should not also rearrange the screen the user is reading.
 *
 * The store lives outside React so a toast can be raised from anywhere,
 * including a callback that has already unmounted its component. It is a
 * module singleton on purpose: there is one screen, so there is one stack.
 */
const listeners = new Set();
let toasts = [];
let nextId = 0;

function publish(next) {
  toasts = next;
  listeners.forEach((listener) => listener());
}

function subscribe(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** How long a toast stays up. Errors linger: they are worth reading twice. */
const DURATION = { error: 8000, warning: 6000, success: 3500, info: 4500 };

/** Beyond this the oldest are dropped, so a burst cannot fill the window. */
const MAX_VISIBLE = 4;

/**
 * Raise a toast.
 *
 * `detail` is the machine's words -- an OS error, a path -- and is rendered as
 * copyable monospace, because the useful thing to do with one is paste it
 * somewhere. `message` is ours.
 *
 * Passing the same `id` twice replaces rather than stacks, so a repeated
 * failure does not pile up four identical cards.
 *
 * Give `titleKey` rather than `title` wherever the caller has no reason to
 * hold a translator. The lookup then happens where the toast is drawn, so a
 * message raised before a language change is still displayed in the language
 * in force when it is read.
 */
export function showToast({ title, titleKey, message, detail, tone = 'info', id }) {
  const key = id ?? `toast-${(nextId += 1)}`;
  const toast = { key, title, titleKey, message, detail, tone };
  const without = toasts.filter((existing) => existing.key !== key);
  publish([...without, toast].slice(-MAX_VISIBLE));
  return key;
}

export function dismissToast(key) {
  publish(toasts.filter((existing) => existing.key !== key));
}

const TONES = {
  info: { icon: FiInfo, accent: 'text-ink-soft' },
  success: { icon: FiCheckCircle, accent: 'text-ok' },
  warning: { icon: FiAlertCircle, accent: 'text-warn' },
  error: { icon: FiAlertCircle, accent: 'text-danger' },
};

const COPIED_RESET_MS = 1200;

function Detail({ text }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(() => {
    navigator.clipboard
      ?.writeText(text)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), COPIED_RESET_MS);
      })
      // Best effort. A clipboard that refuses is not worth a second toast.
      .catch(() => {});
  }, [text]);

  return (
    <button
      type="button"
      onClick={handleCopy}
      className="card-sunken group mt-2 flex w-full items-start gap-2 px-2 py-1.5 text-left transition-colors hover:border-line-strong"
    >
      <span className="min-w-0 flex-1 break-all font-mono text-[11px] leading-relaxed text-ink-soft">
        {text}
      </span>
      {copied ? (
        <FiCheckCircle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-ok" />
      ) : (
        <FiCopy className="mt-0.5 h-3.5 w-3.5 shrink-0 text-ink-faint transition-colors group-hover:text-ink" />
      )}
    </button>
  );
}

function ToastCard({ toast, paused }) {
  const { t } = useTranslation();
  const { icon: Icon, accent } = TONES[toast.tone] ?? TONES.info;
  const [shown, setShown] = useState(false);

  // Mounted off-screen and moved in on the next frame, so the browser has a
  // start value to animate from. Setting both in one paint animates nothing.
  useEffect(() => {
    const frame = requestAnimationFrame(() => setShown(true));
    return () => cancelAnimationFrame(frame);
  }, []);

  // Auto-dismiss, with the remaining time carried across a pause so hovering
  // to read a long error does not restart the clock when the pointer leaves.
  const remaining = useRef(DURATION[toast.tone] ?? DURATION.info);
  const startedAt = useRef(0);
  useEffect(() => {
    if (paused) return undefined;
    startedAt.current = performance.now();
    const timer = setTimeout(() => dismissToast(toast.key), Math.max(0, remaining.current));
    return () => {
      clearTimeout(timer);
      remaining.current -= performance.now() - startedAt.current;
    };
  }, [paused, toast.key]);

  return (
    <div
      className={cn(
        'pointer-events-auto w-[340px] rounded-card border border-line-soft bg-card px-3 py-2.5 shadow-raised',
        'transition-[opacity,transform] duration-200 ease-out',
        shown ? 'translate-x-0 opacity-100' : 'translate-x-6 opacity-0',
      )}
    >
      <div className="flex items-start gap-2.5">
        <Icon className={cn('mt-0.5 h-4 w-4 shrink-0', accent)} strokeWidth={2} />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold leading-5 text-ink">
            {toast.title ?? t(toast.titleKey)}
          </p>
          {toast.message && (
            <p className="mt-0.5 break-words text-xs leading-relaxed text-ink-soft">
              {toast.message}
            </p>
          )}
          {toast.detail && <Detail text={toast.detail} />}
        </div>
        <button
          type="button"
          aria-label="Dismiss"
          onClick={() => dismissToast(toast.key)}
          className="-mr-1 flex h-6 w-6 shrink-0 items-center justify-center rounded-inner text-ink-faint transition-colors duration-fast hover:bg-sunken hover:text-ink"
        >
          <FiX className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

/**
 * The stack itself, portaled to `<body>`.
 *
 * Out of the app shell so it sits above the titlebar and any modal without
 * having to win a z-index argument with either, and so that a page unmounting
 * mid-navigation cannot take a message with it.
 */
export function ToastViewport() {
  const items = useSyncExternalStore(
    subscribe,
    () => toasts,
    () => toasts,
  );
  const [paused, setPaused] = useState(false);

  if (items.length === 0) return null;

  return createPortal(
    <div
      className="pointer-events-none fixed bottom-0 right-0 z-[9999] flex flex-col items-end gap-2 p-4"
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
    >
      {items.map((toast) => (
        <ToastCard key={toast.key} toast={toast} paused={paused} />
      ))}
    </div>,
    document.body,
  );
}
