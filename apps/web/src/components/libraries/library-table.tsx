"use client";

import Link from "next/link";
import useSWR from "swr";
import { BookOpen, ExternalLink, Library as LibraryIcon } from "lucide-react";
import { getLibraries, LIBRARIES_SWR_KEY } from "@/lib/api/libraries-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { DocStatusBadge } from "@/components/libraries/doc-status-badge";
import { LibraryStatsStrip } from "@/components/libraries/library-stats-strip";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export function LibraryTable() {
  const { data: libraries, isLoading } = useSWR(LIBRARIES_SWR_KEY, getLibraries);

  return (
    <div className="flex h-full flex-col">
      {libraries && <LibraryStatsStrip libraries={libraries} />}
      <div className="flex-1 overflow-y-auto">
        <Table>
          <TableHeader className="sticky top-0 bg-canvas">
            <TableRow>
              <TableHead>Library</TableHead>
              <TableHead>Versions indexed</TableHead>
              <TableHead>Documentation</TableHead>
              <TableHead>Changelog</TableHead>
              <TableHead>Used in</TableHead>
              <TableHead>Last indexed</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading && (
              <TableRow>
                <TableCell colSpan={7} className="text-center text-ink-500">
                  Loading…
                </TableCell>
              </TableRow>
            )}
            {!isLoading && libraries?.length === 0 && (
              <TableRow>
                <TableCell colSpan={7} className="text-center text-ink-500">
                  No libraries ingested yet — use &quot;Add library&quot; or run <code className="text-mono-code">agentops sync-docs</code> in a repo.
                </TableCell>
              </TableRow>
            )}
            {libraries?.map((lib) => (
              <TableRow key={lib.slug}>
                <TableCell>
                  <Link href={`/libraries/${encodeURIComponent(lib.slug)}`} className="flex items-center gap-3">
                    <div className="flex size-8 shrink-0 items-center justify-center rounded border border-border-strong bg-panel text-ink-300">
                      <LibraryIcon className="size-4" />
                    </div>
                    <div className="min-w-0">
                      <div className="font-medium text-ink-100">{lib.name}</div>
                      <div className="truncate text-mono-path text-ink-500">{lib.slug}</div>
                    </div>
                  </Link>
                </TableCell>
                <TableCell>
                  <div className="flex flex-wrap gap-1">
                    {lib.versions.length === 0 && <span className="text-mono-code text-ink-500">—</span>}
                    {lib.versions.slice(-3).map((v) => (
                      <span key={v} className="rounded border border-border-strong px-1.5 py-0.5 text-mono-code text-ink-300">
                        {v}
                      </span>
                    ))}
                  </div>
                </TableCell>
                <TableCell>
                  <DocStatusBadge hasMismatch={lib.has_mismatch} />
                </TableCell>
                <TableCell className="text-mono-code text-ink-400">{lib.changelog_versions > 0 ? "Available" : "None"}</TableCell>
                <TableCell className="text-mono-code text-ink-400">
                  {lib.used_in_count} repo{lib.used_in_count === 1 ? "" : "s"}
                </TableCell>
                <TableCell className="text-mono-code text-ink-400">{lib.last_indexed_at ? relativeTimeFromIsoString(lib.last_indexed_at) : "never"}</TableCell>
                <TableCell>
                  <div className="flex justify-end gap-1">
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button variant="outline" size="icon" aria-label="View docs" asChild>
                          <Link href={`/libraries/${encodeURIComponent(lib.slug)}`}>
                            <BookOpen className="size-4" />
                          </Link>
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>View docs</TooltipContent>
                    </Tooltip>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        {lib.github_repo ? (
                          <Button variant="outline" size="icon" aria-label="GitHub" asChild>
                            <a href={lib.github_repo} target="_blank" rel="noreferrer">
                              <ExternalLink className="size-4" />
                            </a>
                          </Button>
                        ) : (
                          <Button variant="outline" size="icon" disabled aria-label="GitHub">
                            <ExternalLink className="size-4" />
                          </Button>
                        )}
                      </TooltipTrigger>
                      <TooltipContent>{lib.github_repo ? "Open on GitHub" : "No GitHub repo registered"}</TooltipContent>
                    </Tooltip>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
