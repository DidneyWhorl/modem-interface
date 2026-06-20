/** @type {import('tailwindcss').Config} */
export default {
  darkMode: 'class',
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  theme: {
    extend: {
      colors: {
        // Signal strength colors (backed by CSS variables for theme switching)
        signal: {
          excellent: 'var(--color-signal-excellent)',
          good: 'var(--color-signal-good)',
          fair: 'var(--color-signal-fair)',
          poor: 'var(--color-signal-poor)',
          none: 'var(--color-signal-none)',
        },
        // Semantic theme colors
        theme: {
          bg: {
            primary: 'var(--color-bg-primary)',
            secondary: 'var(--color-bg-secondary)',
            tertiary: 'var(--color-bg-tertiary)',
            card: 'var(--color-bg-card)',
            input: 'var(--color-bg-input)',
            popover: 'var(--color-bg-popover)',
          },
          text: {
            primary: 'var(--color-text-primary)',
            secondary: 'var(--color-text-secondary)',
            muted: 'var(--color-text-muted)',
            accent: 'var(--color-text-accent)',
          },
          border: {
            DEFAULT: 'var(--color-border)',
            light: 'var(--color-border-light)',
          },
          accent: {
            DEFAULT: 'var(--color-accent)',
            hover: 'var(--color-accent-hover)',
            muted: 'var(--color-accent-muted)',
          },
          success: 'var(--color-success)',
          warning: 'var(--color-warning)',
          error: 'var(--color-error)',
        },
      },
      fontSize: {
        caption: '11px',
      },
      fontFamily: {
        fallen: ['"Share Tech Mono"', 'monospace'],
      },
      animation: {
        'pulse-slow': 'pulse 3s cubic-bezier(0.4, 0, 0.6, 1) infinite',
        'crt-flicker': 'crt-flicker 0.15s infinite',
      },
      keyframes: {
        'crt-flicker': {
          '0%, 100%': { opacity: '0.97' },
          '50%': { opacity: '1' },
        },
      },
    },
  },
  plugins: [],
};
