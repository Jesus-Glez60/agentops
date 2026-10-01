import type { NavShape } from "@/lib/nav-config";

// Small colored marker used in place of an icon for nav rows (sidebar,
// command palette) -- matches the prototype's `item()` helper exactly:
// circle = 50% radius, square = 2px radius, diamond = 2px radius + 45deg
// rotation.
export function NavDot({ color, shape, className }: { color: string; shape: NavShape; className?: string }) {
  return (
    <span
      className={className ?? "size-2 shrink-0"}
      style={{
        background: color,
        borderRadius: shape === "circle" ? "50%" : "2px",
        transform: shape === "diamond" ? "rotate(45deg)" : undefined,
      }}
    />
  );
}
