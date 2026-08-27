import React from 'react';
import { cn } from '../../utils/cn';

const VARIANTS = {
  primary: 'bg-invert text-ink-invert hover:opacity-90',
  accent: 'text-white hover:opacity-90',
  quiet: 'bg-sunken text-ink border border-line-soft hover:border-line-strong',
  ghost: 'text-ink-soft hover:bg-sunken hover:text-ink',
};

const SIZES = {
  sm: 'h-8 px-3 text-xs',
  md: 'h-10 px-4 text-sm',
};

export function Button({
  variant = 'quiet',
  size = 'md',
  className,
  children,
  icon: Icon,
  ...props
}) {
  return (
    <button
      type="button"
      className={cn(
        'inline-flex items-center justify-center gap-2 rounded-pill font-medium',
        'transition-all duration-fast ease-out',
        'disabled:cursor-not-allowed disabled:opacity-45',
        VARIANTS[variant],
        SIZES[size],
        className,
      )}
      style={variant === 'accent' ? { background: 'var(--accent-color)' } : undefined}
      {...props}
    >
      {Icon && <Icon className="h-4 w-4" strokeWidth={2} />}
      {children}
    </button>
  );
}
