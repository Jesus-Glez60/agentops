"use client";

// Shared tool-selection toggle buttons -- used by both the onboarding
// checklist's remote-connect step and the Profile page's "Connect a coding
// tool" section, so the two never drift into two different agent lists or
// two different default selections.
import { Button } from "@/components/ui/button";

export interface AgentOption {
  id: string;
  label: string;
}

/** Mirrors `agentops-cli`'s own `select_agents` interactive list exactly
 * (`named = ["claude", "cursor", "codex", "gemini-cli"]`) -- the server's
 * `GET /connect.sh` route validates `?agents=` against this same set. */
export const AGENT_OPTIONS: AgentOption[] = [
  { id: "claude", label: "Claude Code" },
  { id: "cursor", label: "Cursor" },
  { id: "codex", label: "Codex CLI" },
  { id: "gemini-cli", label: "Gemini CLI" },
];

/** Mirrors `select_agents`'s own defaults (`[true, true, false, false]`) --
 * Claude Code and Cursor pre-selected, Codex CLI and Gemini CLI not. */
export const DEFAULT_SELECTED_AGENTS = ["claude", "cursor"];

export function ToolSelect({ selected, onChange }: { selected: string[]; onChange: (ids: string[]) => void }) {
  function toggle(id: string) {
    onChange(selected.includes(id) ? selected.filter((a) => a !== id) : [...selected, id]);
  }

  return (
    <div className="flex flex-wrap gap-2">
      {AGENT_OPTIONS.map((opt) => (
        <Button key={opt.id} type="button" size="sm" variant={selected.includes(opt.id) ? "default" : "outline"} onClick={() => toggle(opt.id)}>
          {opt.label}
        </Button>
      ))}
    </div>
  );
}
