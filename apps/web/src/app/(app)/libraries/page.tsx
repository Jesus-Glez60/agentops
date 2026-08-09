"use client";

import { useState } from "react";
import Link from "next/link";
import useSWR from "swr";
import { Library as LibraryIcon, Lock, Globe, GitCommitHorizontal } from "lucide-react";
import { getLibraries } from "@/lib/api/docbrain-api";
import { isPrivateVisibility } from "@/lib/api/types";
import { StatCard } from "@/components/shared/stat-card";
import { ErrorState } from "@/components/shared/error-state";
import { EmptyState } from "@/components/shared/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

export default function LibrariesPage() {
  const [orgInput, setOrgInput] = useState("");
  const [org, setOrg] = useState<string | undefined>(undefined);

  const { data: libraries, error, isLoading } = useSWR(["libraries", org], () => getLibraries(org));

  const publicCount = libraries?.filter((l) => !isPrivateVisibility(l.visibility)).length ?? 0;
  const privateCount = (libraries?.length ?? 0) - publicCount;
  const totalVersions = libraries?.reduce((sum, l) => sum + l.versions.length, 0) ?? 0;

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between gap-4">
        <h1 className="text-page-title font-bold">Libraries</h1>
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            setOrg(orgInput.trim() || undefined);
          }}
        >
          <Input value={orgInput} onChange={(e) => setOrgInput(e.target.value)} placeholder="org id (blank = public only)" className="w-64" />
          <Button type="submit" size="sm">
            Filter
          </Button>
        </form>
      </div>

      {error && <ErrorState message={error instanceof Error ? error.message : String(error)} />}

      <div className="grid grid-cols-4 gap-4">
        <StatCard label="Libraries indexed" value={isLoading ? "—" : (libraries?.length ?? 0)} icon={LibraryIcon} />
        <StatCard label="Total versions" value={isLoading ? "—" : totalVersions} icon={GitCommitHorizontal} />
        <StatCard label="Public" value={isLoading ? "—" : publicCount} icon={Globe} />
        <StatCard label="Private (this org)" value={isLoading ? "—" : privateCount} icon={Lock} />
      </div>

      <div className="rounded-md border border-border-strong bg-panel">
        {isLoading && (
          <div className="space-y-2 p-4">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        )}

        {!isLoading && (libraries?.length ?? 0) === 0 && !error && (
          <EmptyState icon={LibraryIcon} title="No libraries visible" description="Ingest one via discover_library or ingest_local_files." />
        )}

        {!isLoading && libraries && libraries.length > 0 && (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Library</TableHead>
                <TableHead>Versions</TableHead>
                <TableHead>Changelog</TableHead>
                <TableHead>Repo</TableHead>
                <TableHead>Docs</TableHead>
                <TableHead>Visibility</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {libraries.map((lib) => (
                <TableRow key={lib.id}>
                  <TableCell>
                    <Link href={`/libraries/${encodeURIComponent(lib.slug)}${org ? `?org=${encodeURIComponent(org)}` : ""}`} className="hover:underline">
                      <span className="font-medium text-ink-100">{lib.name}</span>
                      <span className="ml-2 text-mono-path text-ink-500">{lib.slug}</span>
                    </Link>
                  </TableCell>
                  <TableCell>
                    {lib.versions.length === 0 ? (
                      <span className="text-mono-path text-ink-500">—</span>
                    ) : (
                      <div className="flex flex-wrap gap-1">
                        {lib.versions.map((v) => (
                          <Badge key={v} variant="outline" className="text-mono-code">
                            {v}
                          </Badge>
                        ))}
                      </div>
                    )}
                  </TableCell>
                  <TableCell className="text-body text-ink-300">
                    {lib.changelog_versions > 0 ? `${lib.changelog_versions} entries` : <span className="text-mono-path text-ink-500">—</span>}
                  </TableCell>
                  <TableCell>
                    {lib.github_repo ? (
                      <a href={lib.github_repo} target="_blank" rel="noreferrer" className="text-mono-path text-primary hover:underline">
                        repo
                      </a>
                    ) : (
                      <span className="text-mono-path text-ink-500">—</span>
                    )}
                  </TableCell>
                  <TableCell>
                    {lib.docs_url ? (
                      <a href={lib.docs_url} target="_blank" rel="noreferrer" className="text-mono-path text-primary hover:underline">
                        docs
                      </a>
                    ) : (
                      <span className="text-mono-path text-ink-500">—</span>
                    )}
                  </TableCell>
                  <TableCell>
                    <Badge variant={isPrivateVisibility(lib.visibility) ? "outline" : "secondary"} className="text-mono-code">
                      {isPrivateVisibility(lib.visibility) ? `private (${lib.visibility.Private})` : "public"}
                    </Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>
    </div>
  );
}
