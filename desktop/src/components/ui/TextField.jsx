import React from 'react';
import { cn } from '../../utils/cn';

/**
 * Single-line text input in the sunken style the rest of the shell uses.
 *
 * The browser's own input chrome is turned off rather than restyled: a focus
 * ring drawn by the platform sits at a different radius from every other
 * control here, and on Windows it is a colour the theme does not know about.
 * Focus is shown with the same border the cards use instead.
 */
export function TextField({ label, hint, className, id, ...props }) {
  return (
    <div className="flex flex-col gap-2">
      {label && (
        <label className="text-xs uppercase tracking-wide text-ink-faint" htmlFor={id}>
          {label}
        </label>
      )}
      <input
        id={id}
        type="text"
        spellCheck={false}
        autoComplete="off"
        className={cn(
          'card-sunken h-10 w-full px-3',
          'font-mono text-sm text-ink outline-none',
          'placeholder:text-ink-faint placeholder:font-sans',
          'transition-colors hover:border-line-strong focus:border-line-strong',
          'disabled:cursor-not-allowed disabled:opacity-50',
          className,
        )}
        {...props}
      />
      {hint && <p className="text-xs text-ink-faint">{hint}</p>}
    </div>
  );
}
