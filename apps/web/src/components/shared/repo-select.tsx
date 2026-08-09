"use client";

import useSWR from "swr";
import { getRepos } from "@/lib/api/agentops-api";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/** Last path segment as the friendly display name -- "currentyachts" instead of the full absolute path. */
function repoLabel(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] || path;
}

/**
 * Dropdown over every locally-scanned repo (`GET /repos`), value is still
 * the full absolute path everything else expects -- this only changes what
 * the user has to look at/type, not the underlying scope value. Used
 * everywhere a repo path currently has to be typed/pasted by hand (Search,
 * Knowledge Graph, Documentation).
 */
export function RepoSelect({
  value,
  onChange,
  placeholder = "Select a repo…",
  className,
}: {
  value: string;
  onChange: (path: string) => void;
  placeholder?: string;
  className?: string;
}) {
  const { data: repos, isLoading } = useSWR("repos", getRepos);

  return (
    // Always pass `value` as a defined string (never `undefined`) -- Radix
    // Select treats an `undefined` value prop as "uncontrolled" and a
    // defined one (including "") as "controlled." Switching between the two
    // as `value` goes from empty to populated trips Radix's own
    // controlled/uncontrolled warning; staying controlled from the start
    // avoids it, and the empty string still correctly falls through to the
    // placeholder since it never matches a real SelectItem's value.
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger className={className}>
        <SelectValue placeholder={isLoading ? "Loading repos…" : placeholder}>
          {value ? repoLabel(value) : undefined}
        </SelectValue>
      </SelectTrigger>
      <SelectContent>
        {(repos ?? []).map((r) => (
          <SelectItem key={r.path} value={r.path}>
            <span className="flex min-w-0 flex-col">
              <span className="text-ink-100">{repoLabel(r.path)}</span>
              <span className="truncate text-mono-path text-ink-500">{r.path}</span>
            </span>
          </SelectItem>
        ))}
        {!isLoading && (repos?.length ?? 0) === 0 && <div className="px-2 py-1.5 text-body text-ink-500">No scanned repos yet.</div>}
      </SelectContent>
    </Select>
  );
}
