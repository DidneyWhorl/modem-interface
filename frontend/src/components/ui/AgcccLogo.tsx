/**
 * CTRL-Modem Logo Component
 *
 * Inline SVG recreation of the CTRL-Modem wing/signal logo.
 * Uses currentColor so it adapts to all themes automatically.
 */

interface AgcccLogoProps {
  className?: string;
  size?: number;
  showText?: boolean;
}

export function AgcccLogo({ className = '', size = 32, showText = false }: AgcccLogoProps) {
  return (
    <span className={`inline-flex items-center gap-2 ${className}`}>
      <svg
        width={size}
        height={size}
        viewBox="0 0 64 64"
        fill="currentColor"
        stroke="none"
        className="shrink-0"
      >
        {/* Wing/signal chevron — 3 stacked lines converging to a right-pointing tip */}
        {/* Top line */}
        <path d="M8 22 L40 22 L52 32 L40 26 L8 26 Z" />
        {/* Middle line */}
        <path d="M12 28 L40 28 L52 32 L40 32 L12 32 Z" />
        {/* Bottom chevron */}
        <path d="M16 34 L40 34 L52 32 L40 42 L28 42 Z" />
      </svg>
      {showText && (
        <span
          style={{ fontSize: size * 0.55, fontWeight: 'bold', letterSpacing: 1 }}
          className="font-mono"
        >
          CTRL-Modem
        </span>
      )}
    </span>
  );
}
