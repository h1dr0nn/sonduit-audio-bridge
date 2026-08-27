import React from 'react';
import { FiMinus, FiPlus } from 'react-icons/fi';
import { cn } from '../../utils/cn';

const clamp = (value, min, max) => Math.min(Math.max(value, min), max);

/**
 * Stepper built from plain buttons and a text input. A native
 * `type="number"` draws OS spinners that ignore the design tokens.
 */
export function NumberField({ value, onChange, min = 0, max = 100, step = 1, unit, ariaLabel }) {
  const commit = (next) => {
    if (Number.isNaN(next)) return;
    onChange(clamp(next, min, max));
  };

  const buttonClass = cn(
    'flex h-7 w-7 flex-none items-center justify-center rounded-pill',
    'text-ink-soft transition-colors duration-fast ease-out',
    'hover:bg-card hover:text-ink disabled:cursor-not-allowed disabled:opacity-40',
  );

  return (
    <div
      className={cn(
        'flex h-9 items-center gap-1 rounded-pill border border-line-soft bg-sunken px-1',
        'transition-colors duration-fast ease-out focus-within:border-line-strong',
      )}
    >
      <button
        type="button"
        className={buttonClass}
        onClick={() => commit(value - step)}
        disabled={value <= min}
        aria-label="Decrease"
      >
        <FiMinus className="h-3.5 w-3.5" strokeWidth={2.5} />
      </button>

      <input
        type="text"
        inputMode="numeric"
        aria-label={ariaLabel}
        value={value}
        onChange={(event) => {
          const digits = event.target.value.replace(/[^0-9]/g, '');
          if (digits === '') return;
          commit(Number(digits));
        }}
        className={cn(
          'w-10 border-none bg-transparent p-0 text-center',
          'font-mono text-sm tabular-nums text-ink outline-none',
        )}
      />

      {unit && <span className="pr-1 text-xs text-ink-faint">{unit}</span>}

      <button
        type="button"
        className={buttonClass}
        onClick={() => commit(value + step)}
        disabled={value >= max}
        aria-label="Increase"
      >
        <FiPlus className="h-3.5 w-3.5" strokeWidth={2.5} />
      </button>
    </div>
  );
}
