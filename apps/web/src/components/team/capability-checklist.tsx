import type { Capability } from "@/lib/api/team-api";

/** Grouped by feature area, same grouping `PermissionsMatrixTable` already
 * uses for the read-only display -- rendered here as toggleable checkboxes
 * instead of role columns, for the custom-role creation/edit dialogs. */
export function CapabilityChecklist({ matrix, checked, onToggle }: { matrix: Capability[]; checked: Set<string>; onToggle: (key: string) => void }) {
  const groups = new Map<string, Capability[]>();
  for (const capability of matrix) {
    const group = groups.get(capability.feature_area) ?? [];
    group.push(capability);
    groups.set(capability.feature_area, group);
  }

  return (
    <div className="max-h-72 space-y-4 overflow-y-auto rounded-md border border-border-strong p-3">
      {[...groups.entries()].map(([area, capabilities]) => (
        <div key={area}>
          <p className="mb-1.5 text-mono-code uppercase tracking-wide text-ink-500">{area}</p>
          <div className="space-y-1.5">
            {capabilities.map((c) => (
              <label key={c.key} className="flex cursor-pointer items-center gap-2 text-body text-ink-200">
                <input type="checkbox" checked={checked.has(c.key)} onChange={() => onToggle(c.key)} className="size-3.5 accent-ink-100" />
                {c.label}
              </label>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
