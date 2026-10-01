import type { ReactNode } from "react";
import { LogoMark } from "@/components/shared/logo-mark";

// Prototype's gradient-text treatment for the emphasized phrase inside an
// auth headline (e.g. "a map." in "Let's give your agents a map.") --
// `linear-gradient(95deg, ...)` clipped to text, color stops passed
// per-screen since each auth screen uses a different trio.
export function GradientText({ children, stops }: { children: ReactNode; stops: [string, string, string] }) {
  return (
    <span className="bg-clip-text text-transparent" style={{ backgroundImage: `linear-gradient(95deg, ${stops.join(", ")})` }}>
      {children}
    </span>
  );
}

/**
 * Shared two-panel shell for every auth/onboarding screen (Welcome, Login,
 * Signup, Invite) -- left panel: glow background + logo + huge headline +
 * subtitle (+ optional extra content like a step list or org-preview card),
 * right panel: the actual form, centered, max-width ~420px. Replaces the
 * previous plain centered `Card` layout entirely (redesign plan Phase 2).
 */
export function AuthSplitLayout({
  eyebrow,
  eyebrowColor = "var(--mauve)",
  headline,
  subtitle,
  leftExtra,
  children,
}: {
  eyebrow: string;
  eyebrowColor?: string;
  headline: ReactNode;
  subtitle: string;
  leftExtra?: ReactNode;
  children: ReactNode;
}) {
  return (
    <main className="flex min-h-screen w-full flex-col bg-canvas lg:flex-row">
      <div className="relative flex flex-1 basis-[420px] flex-col gap-10 overflow-hidden px-8 py-8 lg:px-16 lg:py-14">
        {/* Glow background -- purple top-left, peach bottom-right. Same
            radial-gradient treatment the design reuses across every hero
            band; here it's the first and only place it's a full-bleed panel
            background rather than a hero strip. */}
        <div
          aria-hidden
          className="pointer-events-none absolute inset-0"
          style={{
            background: "radial-gradient(circle at 15% 10%, rgba(203,166,247,0.22), transparent 55%), radial-gradient(circle at 85% 90%, rgba(250,179,135,0.18), transparent 55%)",
          }}
        />
        <div className="relative z-10 flex items-center gap-2">
          <LogoMark className="size-[22px] shrink-0" />
          <span className="text-[18px] font-semibold text-ink-100">AgentOps</span>
        </div>
        <div className="relative z-10 flex max-w-[480px] flex-1 flex-col justify-center gap-5">
          <span className="font-mono text-[12px] uppercase tracking-wide" style={{ color: eyebrowColor }}>
            {eyebrow}
          </span>
          <h1 className="text-display-auth font-extrabold tracking-[-0.03em] text-balance text-ink-100">{headline}</h1>
          <p className="text-body-lg text-ink-300">{subtitle}</p>
          {leftExtra}
        </div>
      </div>
      <div className="flex flex-1 items-center justify-center p-6 lg:p-10">
        <div className="w-full max-w-[420px]">{children}</div>
      </div>
    </main>
  );
}
