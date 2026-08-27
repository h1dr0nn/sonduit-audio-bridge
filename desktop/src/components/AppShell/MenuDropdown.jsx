import React, { useEffect, useRef, useState } from 'react';
import { exit } from '@tauri-apps/plugin-process';
import { FiHelpCircle, FiMenu, FiPower, FiRefreshCw, FiSettings } from 'react-icons/fi';
import { cn } from '../../utils/cn';

/** Titlebar menu holding app-level actions. */
export function MenuDropdown({ onNavigate, canRescan, t }) {
  const [open, setOpen] = useState(false);
  const ref = useRef(null);

  useEffect(() => {
    if (!open) return undefined;

    const onPointerDown = (event) => {
      if (!ref.current?.contains(event.target)) setOpen(false);
    };
    const dismiss = () => setOpen(false);

    document.addEventListener('mousedown', onPointerDown);
    window.addEventListener('blur', dismiss);
    window.addEventListener('resize', dismiss);

    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      window.removeEventListener('blur', dismiss);
      window.removeEventListener('resize', dismiss);
    };
  }, [open]);

  const go = (page) => () => {
    setOpen(false);
    onNavigate(page);
  };

  const items = [
    {
      icon: FiRefreshCw,
      label: t('connection.scan'),
      // Nothing to rescan until the bridge core is wired into the shell.
      disabled: !canRescan,
      action: () => setOpen(false),
    },
    { icon: FiSettings, label: t('nav.settings'), action: go('settings') },
    { icon: FiHelpCircle, label: t('nav.about'), action: go('about') },
    {
      icon: FiPower,
      label: t('menu.quit'),
      action: () => {
        setOpen(false);
        exit(0).catch(() => {});
      },
    },
  ];

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        className="titlebar-icon-btn"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t('nav.menu')}
        onClick={() => setOpen((previous) => !previous)}
      >
        <FiMenu className="h-4 w-4" strokeWidth={2} />
      </button>

      {open && (
        <div
          role="menu"
          className={cn(
            'absolute left-0 top-full z-50 mt-1 min-w-48',
            'rounded-inner border border-line-soft bg-card p-1 shadow-raised',
          )}
        >
          {items.map(({ icon: Icon, label, action, disabled }) => (
            <button
              key={label}
              type="button"
              role="menuitem"
              disabled={disabled}
              onClick={action}
              className={cn(
                'flex w-full items-center gap-2.5 rounded-pill px-3 py-2',
                'text-left text-sm transition-colors duration-fast ease-out',
                'disabled:cursor-not-allowed disabled:opacity-40',
                'text-ink-soft enabled:hover:bg-sunken enabled:hover:text-ink',
              )}
            >
              <Icon className="h-4 w-4 flex-none" strokeWidth={1.9} />
              {label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
