import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { exit } from '@tauri-apps/plugin-process';
import { FiCheck, FiChevronRight, FiMenu } from 'react-icons/fi';
import { useSettingsContext } from '../../context/SettingsContext';
import { useBridge } from '../../hooks/useBridge';
import { cn } from '../../utils/cn';
import { Tooltip } from '../ui/Tooltip';

const SEPARATOR = { kind: 'separator' };

/** Breathing room kept between a submenu and the window edge it flips against. */
const EDGE_MARGIN = 8;
const SUBMENU_GAP = 4;

/** Parked off screen until the layout effect has measured and placed the panel. */
const UNPLACED = { position: 'fixed', left: -9999, top: -9999 };

function firstEnabled(items) {
  return items.findIndex((item) => item.kind === 'item' && !item.disabled);
}

function enabledIndices(items) {
  return items.reduce((found, item, index) => {
    if (item.kind === 'item' && !item.disabled) found.push(index);
    return found;
  }, []);
}

function step(indices, current, delta) {
  if (indices.length === 0) return current;
  const at = indices.indexOf(current);
  if (at === -1) return indices[delta > 0 ? 0 : indices.length - 1];
  return indices[(at + delta + indices.length) % indices.length];
}

/**
 * Cascading application menu behind the titlebar hamburger.
 *
 * Every entry maps to something this build can actually do, so the tree is
 * shorter than a conventional File/Edit/View/Help menu: there is no clipboard
 * group because opening the menu blurs the text field the commands would act
 * on, and nothing here opens a URL or checks for updates because the window
 * capability set grants neither.
 *
 * `onOpenPalette` and `onToggleSidebar` are optional on purpose. They carry the
 * only two real keyboard shortcuts in the app, and an entry advertising one is
 * a lie if the handler never arrived, so each is dropped rather than rendered
 * dead.
 */
export function MenuDropdown({ onNavigate, onOpenPalette, onToggleSidebar, t }) {
  const [open, setOpen] = useState(false);
  const [activeTop, setActiveTop] = useState(0);
  const [openTop, setOpenTop] = useState(-1);
  const [activeSub, setActiveSub] = useState(-1);
  const [zone, setZone] = useState('top');
  const [subStyle, setSubStyle] = useState(UNPLACED);

  const rootRef = useRef(null);
  const buttonRef = useRef(null);
  const panelRef = useRef(null);
  const subPanelRef = useRef(null);
  const topRefs = useRef([]);
  const subRefs = useRef([]);

  const { settings, updateSetting } = useSettingsContext();
  const { status, stop } = useBridge();

  const menus = useMemo(() => {
    const go = (page) => () => onNavigate(page);
    const running = status === 'connected' || status === 'connecting';
    const textSize = (value) => ({
      kind: 'item',
      id: `size-${value}`,
      label: t(`common.${value}`),
      checked: settings.fontSize === value,
      run: () => updateSetting('fontSize', value),
    });

    return [
      {
        id: 'file',
        label: t('menu.file'),
        items: [
          {
            kind: 'item',
            id: 'stop-bridge',
            label: t('menu.stopBridge'),
            disabled: !running,
            run: () => {
              stop().catch(() => {});
            },
          },
          SEPARATOR,
          {
            kind: 'item',
            id: 'reload',
            label: t('menu.reloadWindow'),
            run: () => window.location.reload(),
          },
          {
            kind: 'item',
            id: 'quit',
            label: t('menu.quit'),
            run: () => {
              exit(0).catch(() => {});
            },
          },
        ],
      },
      {
        id: 'edit',
        label: t('menu.edit'),
        items: [
          onOpenPalette && {
            kind: 'item',
            id: 'palette',
            label: t('nav.commandPalette'),
            hint: 'Ctrl+K',
            run: onOpenPalette,
          },
          onOpenPalette && SEPARATOR,
          { kind: 'item', id: 'settings', label: t('nav.settings'), run: go('settings') },
        ].filter(Boolean),
      },
      {
        id: 'view',
        label: t('menu.view'),
        items: [
          onToggleSidebar && {
            kind: 'item',
            id: 'sidebar',
            label: t('nav.toggleSidebar'),
            hint: 'Ctrl+B',
            run: onToggleSidebar,
          },
          onToggleSidebar && SEPARATOR,
          { kind: 'item', id: 'go-connection', label: t('nav.connection'), run: go('connection') },
          { kind: 'item', id: 'go-telemetry', label: t('nav.telemetry'), run: go('telemetry') },
          { kind: 'item', id: 'go-editor', label: t('nav.editor'), run: go('editor') },
          SEPARATOR,
          { kind: 'label', id: 'size-label', label: t('settings.fontSize') },
          textSize('small'),
          textSize('medium'),
          textSize('large'),
        ].filter(Boolean),
      },
      {
        id: 'help',
        label: t('menu.help'),
        items: [{ kind: 'item', id: 'about', label: t('nav.about'), run: go('about') }],
      },
    ];
  }, [t, status, stop, settings.fontSize, updateSetting, onNavigate, onOpenPalette, onToggleSidebar]);

  const submenu = openTop === -1 ? null : menus[openTop];

  const close = useCallback((refocus) => {
    setOpen(false);
    setOpenTop(-1);
    setActiveSub(-1);
    setZone('top');
    if (refocus) buttonRef.current?.focus();
  }, []);

  const openSubmenu = useCallback(
    (index, enter) => {
      // Stale entries would otherwise survive into the next menu and be focused
      // in place of a row that no longer exists at that index.
      subRefs.current = [];
      setActiveTop(index);
      setOpenTop(index);

      const first = enter ? firstEnabled(menus[index].items) : -1;
      setActiveSub(first);
      setZone(first === -1 ? 'top' : 'sub');
    },
    [menus],
  );

  useEffect(() => {
    if (!open) return undefined;

    const onPointerDown = (event) => {
      if (!rootRef.current?.contains(event.target)) close(false);
    };
    const dismiss = () => close(false);

    document.addEventListener('mousedown', onPointerDown);
    window.addEventListener('blur', dismiss);
    window.addEventListener('resize', dismiss);

    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      window.removeEventListener('blur', dismiss);
      window.removeEventListener('resize', dismiss);
    };
  }, [open, close]);

  useEffect(() => {
    if (!open) return;
    if (zone === 'sub') subRefs.current[activeSub]?.focus();
    else topRefs.current[activeTop]?.focus();
  }, [open, zone, activeTop, activeSub, openTop]);

  // Placed against the viewport rather than by CSS anchoring, so a submenu that
  // would run past the right or bottom edge flips instead of being clipped.
  // Layout effect, so the move happens before the browser paints the panel.
  useLayoutEffect(() => {
    if (!open || openTop === -1) return;

    const panel = panelRef.current;
    const sub = subPanelRef.current;
    const row = topRefs.current[openTop];
    if (!panel || !sub || !row) return;

    const panelRect = panel.getBoundingClientRect();
    const rowRect = row.getBoundingClientRect();
    const width = sub.offsetWidth;
    const height = sub.offsetHeight;

    let left = panelRect.right + SUBMENU_GAP;
    if (left + width > window.innerWidth - EDGE_MARGIN) {
      left = panelRect.left - width - SUBMENU_GAP;
    }
    left = Math.max(EDGE_MARGIN, left);

    let top = rowRect.top - 4;
    if (top + height > window.innerHeight - EDGE_MARGIN) {
      top = window.innerHeight - EDGE_MARGIN - height;
    }
    top = Math.max(EDGE_MARGIN, top);

    setSubStyle((previous) =>
      previous.left === left && previous.top === top ? previous : { position: 'fixed', left, top },
    );
    // Deliberately not keyed on `menus`: that array is rebuilt on every render,
    // and re-running a measurement that ends in setState would never settle.
  }, [open, openTop]);

  const runItem = (item) => {
    if (item.disabled) return;
    close(true);
    item.run();
  };

  const onKeyDown = (event) => {
    const inSub = zone === 'sub' && submenu;
    const subIndices = submenu ? enabledIndices(submenu.items) : [];

    switch (event.key) {
      case 'Escape':
        event.preventDefault();
        if (openTop !== -1) {
          setOpenTop(-1);
          setActiveSub(-1);
          setZone('top');
        } else {
          close(true);
        }
        break;
      case 'ArrowDown':
      case 'ArrowUp': {
        event.preventDefault();
        const delta = event.key === 'ArrowDown' ? 1 : -1;
        if (inSub) {
          setActiveSub((index) => step(subIndices, index, delta));
        } else {
          const next = (activeTop + delta + menus.length) % menus.length;
          if (openTop === -1) setActiveTop(next);
          else openSubmenu(next, false);
        }
        break;
      }
      case 'ArrowRight':
        if (!inSub) {
          event.preventDefault();
          openSubmenu(activeTop, true);
        }
        break;
      case 'ArrowLeft':
        if (inSub) {
          event.preventDefault();
          setOpenTop(-1);
          setActiveSub(-1);
          setZone('top');
        }
        break;
      case 'Home':
      case 'End': {
        event.preventDefault();
        const toLast = event.key === 'End';
        if (inSub) {
          setActiveSub(subIndices[toLast ? subIndices.length - 1 : 0] ?? -1);
        } else if (openTop === -1) {
          setActiveTop(toLast ? menus.length - 1 : 0);
        } else {
          openSubmenu(toLast ? menus.length - 1 : 0, false);
        }
        break;
      }
      case 'Enter':
      case ' ':
        // Without this the browser also fires the button's click, activating
        // the row a second time.
        event.preventDefault();
        if (inSub) runItem(submenu.items[activeSub]);
        else openSubmenu(activeTop, true);
        break;
      case 'Tab':
        close(false);
        break;
      default:
        break;
    }
  };

  return (
    <div ref={rootRef} className="relative">
      {/* Suppressed while the menu is open: the panel covers the spot the tip
        * would take, and a tip over an open menu reads as a stuck overlay. */}
      <Tooltip label={t('nav.menu')} disabled={open}>
        <button
          ref={buttonRef}
          type="button"
          className="titlebar-icon-btn"
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={t('nav.menu')}
          onClick={() => {
            if (open) {
              close(true);
            } else {
              setActiveTop(0);
              setOpenTop(-1);
              setActiveSub(-1);
              setZone('top');
              setSubStyle(UNPLACED);
              setOpen(true);
            }
          }}
        >
          <FiMenu className="h-4 w-4" strokeWidth={2} />
        </button>
      </Tooltip>

      {open && (
        <div
          ref={panelRef}
          role="menu"
          aria-label={t('nav.menu')}
          onKeyDown={onKeyDown}
          className={cn(
            'absolute left-0 top-full z-50 mt-1 min-w-36',
            'rounded-inner border border-line-soft bg-card p-1 shadow-raised',
          )}
        >
          {menus.map((menu, index) => (
            // No mouseleave anywhere in this tree: a submenu is closed only by
            // another top-level row opening, so the pointer can cross the gap
            // between the two panels without the target vanishing under it.
            <div key={menu.id} role="none" className="relative">
              <button
                ref={(element) => {
                  topRefs.current[index] = element;
                }}
                type="button"
                role="menuitem"
                aria-haspopup="menu"
                aria-expanded={openTop === index}
                tabIndex={zone === 'top' && activeTop === index ? 0 : -1}
                onMouseEnter={() => openSubmenu(index, false)}
                onClick={() => {
                  if (openTop === index) {
                    setOpenTop(-1);
                    setActiveSub(-1);
                    setZone('top');
                  } else {
                    openSubmenu(index, false);
                  }
                }}
                className={cn(
                  'flex w-full items-center gap-3 rounded-pill px-3 py-2',
                  'text-left text-sm outline-none transition-colors duration-fast ease-out',
                  openTop === index || activeTop === index
                    ? 'bg-sunken text-ink'
                    : 'text-ink-soft hover:bg-sunken hover:text-ink',
                )}
              >
                <span className="flex-1">{menu.label}</span>
                <FiChevronRight className="h-3.5 w-3.5 flex-none text-ink-faint" strokeWidth={2} />
              </button>

              {openTop === index && (
                <div
                  ref={subPanelRef}
                  role="menu"
                  aria-label={menu.label}
                  style={subStyle}
                  className={cn(
                    'z-50 min-w-52 rounded-inner border border-line-soft bg-card p-1 shadow-raised',
                  )}
                >
                  {menu.items.map((item, itemIndex) => {
                    if (item.kind === 'separator') {
                      return (
                        <div
                          key={`separator-${itemIndex}`}
                          role="separator"
                          className="my-1 h-px bg-line-soft"
                        />
                      );
                    }

                    if (item.kind === 'label') {
                      return (
                        <p
                          key={item.id}
                          className="px-3 pb-1 pt-2 text-xs font-medium uppercase tracking-wide text-ink-faint"
                        >
                          {item.label}
                        </p>
                      );
                    }

                    const radio = item.checked !== undefined;

                    return (
                      <button
                        key={item.id}
                        ref={(element) => {
                          subRefs.current[itemIndex] = element;
                        }}
                        type="button"
                        role={radio ? 'menuitemradio' : 'menuitem'}
                        aria-checked={radio ? item.checked : undefined}
                        disabled={item.disabled}
                        tabIndex={zone === 'sub' && activeSub === itemIndex ? 0 : -1}
                        onMouseEnter={() => {
                          if (item.disabled) return;
                          setActiveSub(itemIndex);
                          setZone('sub');
                        }}
                        onClick={() => runItem(item)}
                        className={cn(
                          'flex w-full items-center gap-6 rounded-pill px-3 py-2',
                          'text-left text-sm outline-none transition-colors duration-fast ease-out',
                          'disabled:cursor-not-allowed disabled:opacity-40',
                          zone === 'sub' && activeSub === itemIndex
                            ? 'bg-sunken text-ink'
                            : 'text-ink-soft enabled:hover:bg-sunken enabled:hover:text-ink',
                        )}
                      >
                        <span className="flex-1 whitespace-nowrap">{item.label}</span>
                        {item.hint && (
                          <kbd className="flex-none rounded border border-line-soft px-1.5 py-0.5 font-mono text-xs text-ink-faint">
                            {item.hint}
                          </kbd>
                        )}
                        {radio && (
                          <FiCheck
                            className={cn(
                              'h-3.5 w-3.5 flex-none',
                              item.checked ? 'text-ink' : 'opacity-0',
                            )}
                            strokeWidth={2.2}
                          />
                        )}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
