import type { LucideIcon } from "lucide-react";

export function EmptyState({ icon: Icon, title, description }: { icon: LucideIcon; title: string; description?: string }) {
  return (
    <div className="flex h-full min-h-[60vh] flex-col items-center justify-center gap-2 p-8 text-center">
      <Icon className="size-8 text-ink-500" />
      <p className="text-subheading text-ink-100">{title}</p>
      {description && <p className="text-body text-ink-500">{description}</p>}
    </div>
  );
}
