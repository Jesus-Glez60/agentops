"use client";

import useSWR from "swr";
import { ChevronDown } from "lucide-react";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/agentops-api";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

/** `selected` empty means "all repos" -- the same "no forced scope" default as search itself. */
export function ScopeSelector({
  selected,
  onChange,
}: {
  selected: string[];
  onChange: (repos: string[]) => void;
}) {
  // Same SWR key the Overview page's repo table and the topbar already use --
  // one shared cache entry, no new fetch.
  const { data: repos } = useSWR(REPOS_SWR_KEY, getRepos);

  function toggle(name: string) {
    onChange(selected.includes(name) ? selected.filter((r) => r !== name) : [...selected, name]);
  }

  const label = selected.length === 0 ? "All repositories" : selected.length === 1 ? selected[0] : `${selected.length} repositories`;

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button variant="outline" size="sm" className="gap-1.5 text-section">
          {label}
          <ChevronDown className="size-3.5 text-ink-500" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64">
        {!repos ? (
          <p className="px-1 py-1 text-body text-ink-500">Loading repositories…</p>
        ) : repos.length === 0 ? (
          <p className="px-1 py-1 text-body text-ink-500">No repositories scanned yet.</p>
        ) : (
          <div className="flex flex-col gap-1">
            {selected.length > 0 && (
              <button type="button" onClick={() => onChange([])} className="self-start px-1 text-mono-code text-primary hover:underline">
                Reset to all
              </button>
            )}
            {repos.map((repo) => (
              <label key={repo.name} className="flex items-center gap-2 rounded-md px-1 py-1 text-body text-ink-300 hover:bg-raised">
                <Checkbox checked={selected.includes(repo.name)} onCheckedChange={() => toggle(repo.name)} />
                {repo.name}
              </label>
            ))}
          </div>
        )}
      </PopoverContent>
    </Popover>
  );
}
