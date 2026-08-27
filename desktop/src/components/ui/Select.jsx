import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { FiCheck, FiChevronDown } from 'react-icons/fi';
import { cn } from '../../utils/cn';

/**
 * Custom listbox. Native `<select>` renders with OS chrome that ignores the
 * design tokens, so every dropdown in the app uses this instead.
 *
 * Keyboard model follows the ARIA listbox pattern: Up/Down move the active
 * option, Enter or Space commits it, Escape closes without committing, Home
 * and End jump to the ends.
 */
export function Select({ value, options, onChange, className, ariaLabel }) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const rootRef = useRef(null);
  const listRef = useRef(null);
  const listId = useId();

  const selectedIndex = options.findIndex((option) => option.value === value);
  const selected = selectedIndex >= 0 ? options[selectedIndex] : null;

  const close = useCallback(() => setOpen(false), []);

  const commit = useCallback(
    (index) => {
      const option = options[index];
      if (option) onChange(option.value);
      setOpen(false);
    },
    [onChange, options],
  );

  useEffect(() => {
    if (!open) return undefined;

    setActiveIndex(selectedIndex >= 0 ? selectedIndex : 0);

    const onPointerDown = (event) => {
      if (!rootRef.current?.contains(event.target)) close();
    };
    // A scroll or resize would leave the popup detached from its trigger.
    document.addEventListener('mousedown', onPointerDown);
    window.addEventListener('resize', close);

    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      window.removeEventListener('resize', close);
    };
    // selectedIndex is read only to seed the highlight when the popup opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, close]);

  useEffect(() => {
    if (!open || !listRef.current) return;
    const node = listRef.current.children[activeIndex];
    node?.scrollIntoView({ block: 'nearest' });
  }, [open, activeIndex]);

  const onKeyDown = (event) => {
    if (!open) {
      if (event.key === 'Enter' || event.key === ' ' || event.key === 'ArrowDown') {
        event.preventDefault();
        setOpen(true);
      }
      return;
    }

    switch (event.key) {
      case 'Escape':
        event.preventDefault();
        close();
        break;
      case 'ArrowDown':
        event.preventDefault();
        setActiveIndex((index) => Math.min(index + 1, options.length - 1));
        break;
      case 'ArrowUp':
        event.preventDefault();
        setActiveIndex((index) => Math.max(index - 1, 0));
        break;
      case 'Home':
        event.preventDefault();
        setActiveIndex(0);
        break;
      case 'End':
        event.preventDefault();
        setActiveIndex(options.length - 1);
        break;
      case 'Enter':
      case ' ':
        event.preventDefault();
        commit(activeIndex);
        break;
      default:
        break;
    }
  };

  return (
    <div ref={rootRef} className={cn('relative', className)}>
      <button
        type="button"
        role="combobox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-haspopup="listbox"
        aria-label={ariaLabel}
        onClick={() => setOpen((previous) => !previous)}
        onKeyDown={onKeyDown}
        className={cn(
          'flex h-9 w-full items-center justify-between gap-2 rounded-pill',
          'border border-line-soft bg-sunken px-3.5 text-sm text-ink',
          'transition-colors duration-fast ease-out',
          'hover:border-line-strong focus-visible:outline-none',
          open && 'border-line-strong',
        )}
      >
        <span className="truncate">{selected?.label ?? ''}</span>
        <FiChevronDown
          className={cn(
            'h-4 w-4 flex-none text-ink-faint transition-transform duration-fast ease-out',
            open && 'rotate-180',
          )}
          strokeWidth={2}
        />
      </button>

      {open && (
        <ul
          id={listId}
          ref={listRef}
          role="listbox"
          tabIndex={-1}
          aria-label={ariaLabel}
          className={cn(
            'scroll-area absolute right-0 z-50 mt-1.5 max-h-60 min-w-full',
            'rounded-inner border border-line-soft bg-card p-1 shadow-raised',
          )}
        >
          {options.map((option, index) => {
            const isSelected = option.value === value;
            return (
              <li key={option.value}>
                <button
                  type="button"
                  role="option"
                  aria-selected={isSelected}
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => commit(index)}
                  className={cn(
                    'flex w-full items-center justify-between gap-3 rounded-pill',
                    'px-3 py-2 text-left text-sm transition-colors duration-fast ease-out',
                    index === activeIndex ? 'bg-sunken text-ink' : 'text-ink-soft',
                  )}
                >
                  <span className="truncate">{option.label}</span>
                  {isSelected && (
                    <FiCheck className="h-4 w-4 flex-none text-accent" strokeWidth={2.25} />
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
