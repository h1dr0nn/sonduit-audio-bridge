/** @type {import('tailwindcss').Config} */
export default {
  darkMode: 'class',
  content: ['./index.html', './src/**/*.{js,jsx,ts,tsx}'],
  theme: {
    fontSize: {
      xs: ['var(--text-xs)', '1.4'],
      sm: ['var(--text-sm)', '1.45'],
      base: ['var(--text-base)', '1.5'],
      lg: ['var(--text-lg)', '1.45'],
      '2xl': ['var(--text-2xl)', '1.25'],
      xl: ['var(--text-xl)', '1.35'],
      '3xl': ['var(--text-3xl)', '1.15'],
      '4xl': ['var(--text-4xl)', '1.1'],
    },
    extend: {
      colors: {
        app: 'var(--surface-app)',
        card: 'var(--surface-card)',
        sunken: 'var(--surface-sunken)',
        rail: 'var(--surface-rail)',
        invert: 'var(--surface-invert)',
        accent: 'var(--accent-color)',
        'accent-soft': 'var(--accent-soft)',
        ok: 'var(--ok)',
        warn: 'var(--warn)',
        danger: 'var(--danger)',
        ink: {
          DEFAULT: 'var(--text-primary)',
          soft: 'var(--text-secondary)',
          faint: 'var(--text-tertiary)',
          invert: 'var(--text-on-invert)',
        },
        line: {
          soft: 'var(--line-soft)',
          strong: 'var(--line-strong)',
        },
      },
      borderRadius: {
        shell: 'var(--radius-shell)',
        card: 'var(--radius-card)',
        inner: 'var(--radius-inner)',
        pill: 'var(--radius-pill)',
      },
      boxShadow: {
        card: 'var(--shadow-card)',
        raised: 'var(--shadow-raised)',
      },
      transitionTimingFunction: {
        out: 'var(--ease-out)',
      },
      transitionDuration: {
        fast: '140ms',
        normal: '200ms',
      },
      fontFamily: {
        sans: 'var(--font-sans)',
        mono: 'var(--font-mono)',
      },
    },
  },
  plugins: [],
};
