"use client";

import { use, useState } from "react";
import { useSearchParams } from "next/navigation";
import useSWR from "swr";
import { callDocbrainTool, getLibraries, probeDocVersions } from "@/lib/api/docbrain-api";
import { DocContent } from "@/components/docs/doc-content";
import { ErrorState } from "@/components/shared/error-state";
import { EmptyState } from "@/components/shared/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Badge } from "@/components/ui/badge";
import { FileQuestion } from "lucide-react";
import { isPrivateVisibility } from "@/lib/api/types";

export default function LibraryDetailPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = use(params);
  const searchParams = useSearchParams();
  const org = searchParams.get("org") ?? undefined;

  const { data: libraries, error: libError, isLoading: libLoading } = useSWR(["libraries", org], () => getLibraries(org));
  const library = libraries?.find((l) => l.slug === slug);

  const { data: versions, isLoading: versionsLoading } = useSWR(["doc-versions", slug, org], () => probeDocVersions(slug, org));

  const [version, setVersion] = useState<string | null>(null);
  const activeVersion = version ?? versions?.[0] ?? null;

  const { data: docsResult, isLoading: docsLoading } = useSWR(
    activeVersion ? ["get_docs", slug, activeVersion, org] : null,
    () => callDocbrainTool("get_docs", { slug, version: activeVersion, org }),
  );

  const [fromVersion, setFromVersion] = useState<string | null>(null);
  const [toVersion, setToVersion] = useState<string | null>(null);
  const { data: changelogResult } = useSWR(
    fromVersion && toVersion ? ["get_changelog", slug, fromVersion, toVersion, org] : null,
    () => callDocbrainTool("get_changelog", { slug, from_version: fromVersion, to_version: toVersion, org }),
  );

  if (libLoading || versionsLoading) return <Skeleton className="h-96 w-full" />;
  if (libError) return <ErrorState message={libError instanceof Error ? libError.message : String(libError)} />;
  if (!library) return <EmptyState icon={FileQuestion} title="Library not visible" description={`No library '${slug}' visible to this caller.`} />;

  return (
    <div className="flex flex-col gap-4">
      <div>
        <div className="flex items-center gap-2">
          <h1 className="text-page-title font-bold">{library.name}</h1>
          <Badge variant={isPrivateVisibility(library.visibility) ? "outline" : "secondary"} className="text-mono-code">
            {isPrivateVisibility(library.visibility) ? `private (${library.visibility.Private})` : "public"}
          </Badge>
        </div>
        <div className="mt-1 flex gap-3 text-mono-path text-ink-500">
          {library.github_repo && (
            <a href={library.github_repo} target="_blank" rel="noreferrer" className="hover:underline">
              {library.github_repo}
            </a>
          )}
          {library.docs_url && (
            <a href={library.docs_url} target="_blank" rel="noreferrer" className="hover:underline">
              {library.docs_url}
            </a>
          )}
        </div>
      </div>

      {versions && versions.length === 0 && (
        <EmptyState icon={FileQuestion} title="No docs scraped yet" description="Run scrape_library or ingest_local_files for this library." />
      )}

      {versions && versions.length > 0 && (
        <Tabs defaultValue="docs">
          <div className="flex items-center justify-between">
            <TabsList>
              <TabsTrigger value="docs">Documentation</TabsTrigger>
              <TabsTrigger value="changelog">Changelog</TabsTrigger>
            </TabsList>
            <Select value={activeVersion ?? undefined} onValueChange={setVersion}>
              <SelectTrigger className="w-40">
                <SelectValue placeholder="Version" />
              </SelectTrigger>
              <SelectContent>
                {versions.map((v) => (
                  <SelectItem key={v} value={v}>
                    {v}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <TabsContent value="docs" className="mt-4">
            {docsLoading && <Skeleton className="h-64 w-full" />}
            {docsResult && <DocContent markdown={docsResult.content[0]?.text ?? ""} />}
          </TabsContent>

          <TabsContent value="changelog" className="mt-4 flex flex-col gap-3">
            <div className="flex items-center gap-2 text-body text-ink-300">
              From
              <Select value={fromVersion ?? undefined} onValueChange={setFromVersion}>
                <SelectTrigger className="w-32">
                  <SelectValue placeholder="version" />
                </SelectTrigger>
                <SelectContent>
                  {versions.map((v) => (
                    <SelectItem key={v} value={v}>
                      {v}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              to
              <Select value={toVersion ?? undefined} onValueChange={setToVersion}>
                <SelectTrigger className="w-32">
                  <SelectValue placeholder="version" />
                </SelectTrigger>
                <SelectContent>
                  {versions.map((v) => (
                    <SelectItem key={v} value={v}>
                      {v}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {changelogResult && (
              <pre className="whitespace-pre-wrap rounded-md border border-border-strong bg-raised p-3 text-body text-ink-300">
                {changelogResult.content[0]?.text}
              </pre>
            )}
          </TabsContent>
        </Tabs>
      )}

      <div className="rounded-md border border-dashed border-border-strong p-4 text-body text-ink-500">
        Version-mismatch detection and &quot;repositories using this&quot; (with each repo&apos;s installed version) aren&apos;t tracked yet
        — this needs a join between a connected repo&apos;s exact dependency versions and docbrain&apos;s tracked versions that doesn&apos;t
        exist in the backend today.
      </div>
    </div>
  );
}
