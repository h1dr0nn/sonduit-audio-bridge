import React, { useEffect, useMemo, useRef, useState } from 'react';
import {
  FiActivity,
  FiCornerDownLeft,
  FiHelpCircle,
  FiMoon,
  FiRadio,
  FiRefreshCw,
  FiSearch,
  FiSettings,
  FiSidebar,
  FiSliders,
  FiSun,
} from 'react-icons/fi';
import { cn } from '../../utils/cn';

/**
 * Fuzzy-ish match: every character of the query must appear in order. Cheap,
 * dependency free, and good enough for a list this size.
 */
function matches(haystack, query) {
  if (query === '') return true;
  const text = haystack.toLowerCase();
  const needle = query.toLowerCase();
  let at = 0;
  for (const character of needle) {
    at = text.indexOf(character, at);
    if (at === -1) return false;
    at += 1;
  }
  return true;
}

export function CommandPalette({
  open,
  onClose,
  onNavigate,
  onToggleSidebar,
  onSetTheme,
  theme,
  canRescan,
  t,
}) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const inputRef = useRef(null);
  const listRef = useRef(null);

  const items = useMemo(() => {
    const go = (page) => () => {
      onNavigate(page);
      onClose();
    };

    return [
      {
        id: 'go-connection',
        section: t('palette.navigate'),
        label: t('nav.connection'),
        icon: FiRadio,
        run: go('connection'),
      },
      {
        id: 'go-telemetry',
        section: t('palette.navigate'),
        label: t('nav.telemetry'),
        icon: FiActivity,
        run: go('telemetry'),
      },
      {
        id: 'go-editor',
        section: t('palette.navigate'),
        label: t('nav.editor'),
        icon: FiSliders,
        run: go('editor'),
      },
      {
        id: 'go-settings',
        section: t('palette.navigate'),
        label: t('nav.settings'),
        icon: FiSettings,
        run: go('settings'),
      },
      {
        id: 'go-about',
        section: t('palette.navigate'),
        label: t('nav.about'),
        icon: FiHelpCircle,
        run: go('about'),
      },
      {
        id: 'act-scan',
        section: t('palette.actions'),
        label: t('connection.scan'),
        icon: FiRefreshCw,
        // Nothing to rescan until the bridge core is wired in.
        disabled: !canRescan,
        run: onClose,
      },
      {
        id: 'act-sidebar',
        section: t('palette.actions'),
        label: t('nav.toggleSidebar'),
        icon: FiSidebar,
        hint: 'Ctrl+B',
        run: () => {
          onToggleSidebar();
          onClose();
        },
      },
      {
        id: 'act-theme',
        section: t('palette.actions'),
        label: theme === 'dark' ? t('common.light') : t('common.dark'),
        icon: theme === 'dark' ? FiSun : FiMoon,
        run: () => {
          onSetTheme(theme === 'dark' ? 'light' : 'dark');
          onClose();
        },
      },
    ];
  }, [t, theme, canRescan, onNavigate, onToggleSidebar, onSetTheme, onClose]);

  const visible = useMemo(
    () => items.filter((item) => matches(`${item.section} ${item.label}`, query)),
    [items, query],
  );

  useEffect(() => {
    if (!open) return undefined;
    setQuery('');
    setActive(0);
    // Focus after paint, or the input is not in the document yet.
    const id = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(id);
  }, [open]);

  useEffect(() => {
    setActive(0);
  }, [query]);

  useEffect(() => {
    if (!open || !listRef.current) return;
    listRef.current.children[active]?.scrollIntoView({ block: 'nearest' });
  }, [open, active]);

  if (!open) return null;

  const commit = (index) => {
    const item = visible[index];
    if (item && !item.disabled) item.run();
  };

  const onKeyDown = (event) => {
    switch (event.key) {
      case 'Escape':
        event.preventDefault();
        onClose();
        break;
      case 'ArrowDown':
        event.preventDefault();
        setActive((index) => Math.min(index + 1, visible.length - 1));
        break;
      case 'ArrowUp':
        event.preventDefault();
        setActive((index) => Math.max(index - 1, 0));
        break;
      case 'Home':
        event.preventDefault();
        setActive(0);
        break;
      case 'End':
        event.preventDefault();
        setActive(visible.length - 1);
        break;
      case 'Enter':
        event.preventDefault();
        commit(active);
        break;
      default:
        break;
    }
  };

  let lastSection = null;

  return (
    <>
      {/* Full-bleed backdrop, exactly as adb-compass does it. It deliberately
        * covers the titlebar area too: the titlebar sits above it in the
        * stacking order, so the chrome stays crisp and draggable while the
        * dimming reads as one continuous surface. Cutting the backdrop off at
        * the titlebar instead leaves a visible seam. */}
      <div className="modal-backdrop" role="presentation" onMouseDown={onClose} />

      {/* Positioned rather than flex-centred, so the panel keeps a fixed
        * distance below the titlebar regardless of its own height. */}
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t('nav.commandPalette')}
        className="modal-panel w-[560px] max-w-[90vw] overflow-hidden rounded-card"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="flex items-center gap-3 border-b border-line-soft px-4">
          <FiSearch className="h-4 w-4 flex-none text-ink-faint" strokeWidth={2} />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder={t('palette.placeholder')}
            aria-label={t('palette.placeholder')}
            className={cn(
              'h-12 w-full border-none bg-transparent p-0 text-sm text-ink',
              'outline-none placeholder:text-ink-faint',
            )}
          />
        </div>

        {visible.length === 0 ? (
          <p className="px-4 py-8 text-center text-sm text-ink-soft">{t('palette.noResults')}</p>
        ) : (
          <ul ref={listRef} role="listbox" className="scroll-area max-h-[340px] p-2">
            {visible.map((item, index) => {
              const Icon = item.icon;
              const showSection = item.section !== lastSection;
              lastSection = item.section;

              return (
                <li key={item.id}>
                  {showSection && (
                    <p className="px-3 pb-1 pt-2 text-xs font-medium uppercase tracking-wide text-ink-faint">
                      {item.section}
                    </p>
                  )}
                  <button
                    type="button"
                    role="option"
                    aria-selected={index === active}
                    disabled={item.disabled}
                    onMouseEnter={() => setActive(index)}
                    onClick={() => commit(index)}
                    className={cn(
                      'flex w-full items-center gap-3 rounded-inner px-3 py-2 text-left text-sm',
                      'transition-colors duration-fast ease-out',
                      'disabled:cursor-not-allowed disabled:opacity-40',
                      index === active && !item.disabled ? 'bg-sunken text-ink' : 'text-ink-soft',
                    )}
                  >
                    <Icon className="h-4 w-4 flex-none" strokeWidth={1.9} />
                    <span className="flex-1 truncate">{item.label}</span>
                    {item.hint && (
                      <kbd className="rounded border border-line-soft px-1.5 py-0.5 font-mono text-xs text-ink-faint">
                        {item.hint}
                      </kbd>
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
        )}

        <div className="flex items-center gap-4 border-t border-line-soft px-4 py-2 text-xs text-ink-faint">
          <span className="flex items-center gap-1.5">
            <FiCornerDownLeft className="h-3 w-3" strokeWidth={2} />
            {t('palette.select')}
          </span>
          <span>{t('palette.navigateHint')}</span>
          <span className="ml-auto">Esc</span>
        </div>
      </div>
    </>
  );
}
