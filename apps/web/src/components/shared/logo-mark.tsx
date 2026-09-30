// The AgentOps brand mark: a 22x22 square, a peach dot, and a connecting
// line -- matches apps/web/src/app/icon.svg exactly (one source geometry
// for both the favicon and every in-app placement, so they can't drift).
export function LogoMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 22 22" className={className} aria-hidden="true">
      <rect x="0" y="0" width="9" height="9" fill="#cdd6f4" />
      <circle cx="17" cy="17" r="5" fill="#fab387" />
      <line x1="4" y1="4" x2="14.6" y2="14.6" stroke="#cdd6f4" strokeWidth="1.5" />
    </svg>
  );
}
