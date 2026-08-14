"use client";

import { Suspense, useState } from "react";
import Link from "next/link";
import { useParams, useRouter, useSearchParams } from "next/navigation";
import useSWR from "swr";
import { toast } from "sonner";
import { ExternalLink, RefreshCw, Search } from "lucide-react";
import { getLibrary, getLibraryChangelog, getLibraryDocs, rescrapeLibrary, LIBRARIES_SWR_KEY } from "@/lib/api/libraries-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { MarkdownContent } from "@/components/shared/markdown-content";
import { DocStatusBadge } from "@/components/libraries/doc-status-badge";
import { ReposUsingThisList } from "@/components/libraries/repos-using-this-list";
import { VersionMismatchBanner } from "@/components/libraries/version-mismatch-banner";
import { VersionPillSelector } from "@/components/libraries/version-pill-selector";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export default function LibraryDetailPage() {
  // useSearchParams requires a Suspense boundary during static generation.
  return (
    <Suspense fallback={null}>
      <LibraryDetailPageInner />
    </Suspense>
  );
}

function LibraryDetailPageInner() {
  const { slug } = useParams<{ slug: string }>();
  const searchParams = useSearchParams();
  const router = useRouter();
  const [tab, setTab] = useState("docs");

  const { data, isLoading } = useSWR(slug ? [LIBRARIES_SWR_KEY, slug] : null, () => getLibrary(slug));
  const library = data?.library;
  const usedIn = data?.used_in ?? [];

  const latestVersion = library?.versions.at(-1) ?? null;
  const viewingVersion = searchParams.get("version") ?? latestVersion;

  function selectVersion(v: string) {
    router.replace(`/libraries/${encodeURIComponent(slug)}?version=${encodeURIComponent(v)}`);
  }

  if (isLoading) {
    return <p className="p-8 text-body text-ink-500">Loading…</p>;
  }
  if (!library || !viewingVersion) {
    return <p className="p-8 text-body text-ink-500">No library named &quot;{slug}&quot; is registered.</p>;
  }

  const mismatchedAgainstViewing = usedIn.filter((u) => u.declared_version !== viewingVersion);

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-[52px] shrink-0 items-center justify-between border-b border-border-strong px-5">
        <div className="flex items-center gap-2 text-section">
          <Link href="/libraries" className="text-ink-400 transition-colors hover:text-ink-100">
            Libraries
          </Link>
          <span className="text-ink-600">/</span>
          <span className="font-medium text-ink-100">{library.name}</span>
        </div>
        {library.github_repo && (
          <a href={library.github_repo} target="_blank" rel="noreferrer" className="flex items-center gap-1.5 text-section text-ink-400 transition-colors hover:text-ink-100">
            <ExternalLink className="size-3.5" />
            {library.github_repo.replace(/^https?:\/\/(www\.)?github\.com\//, "")}
          </a>
        )}
      </div>

      <div className="flex shrink-0 items-start justify-between gap-4 border-b border-border-strong px-6 py-4">
        <div>
          <div className="mb-0.5 flex items-center gap-2">
            <h1 className="text-lg font-semibold text-ink-100">{library.name}</h1>
            <DocStatusBadge hasMismatch={library.has_mismatch} />
          </div>
          {library.description && <p className="max-w-xl text-body text-ink-400">{library.description}</p>}
          <p className="mt-0.5 text-mono-code text-ink-500">{library.slug}</p>
        </div>
        {library.versions.length > 0 && (
          <div className="flex flex-col items-end gap-2">
            <p className="text-mono-code uppercase text-ink-500">Viewing version</p>
            <VersionPillSelector versions={library.versions} selected={viewingVersion} onSelect={selectVersion} />
          </div>
        )}
      </div>

      <VersionMismatchBanner viewingVersion={viewingVersion} mismatched={mismatchedAgainstViewing} />

      <Tabs value={tab} onValueChange={setTab} className="min-h-0 flex-1">
        <TabsList variant="line" className="shrink-0 border-b border-border-strong px-6">
          <TabsTrigger value="docs">Documentation</TabsTrigger>
          <TabsTrigger value="changelog">Changelog</TabsTrigger>
          <TabsTrigger value="repos">Repositories using this ({usedIn.length})</TabsTrigger>
        </TabsList>

        <div className="flex min-h-0 flex-1 overflow-hidden">
          <div className="flex-1 overflow-y-auto px-8 py-6">
            <TabsContent value="docs" className="mt-0">
              <LibraryDocsPane slug={slug} version={viewingVersion} />
            </TabsContent>
            <TabsContent value="changelog" className="mt-0">
              <LibraryChangelogPane slug={slug} />
            </TabsContent>
            <TabsContent value="repos" className="mt-0">
              <ReposUsingThisList usedIn={usedIn} />
            </TabsContent>
          </div>

          <div className="w-[260px] shrink-0 space-y-4 overflow-y-auto border-l border-border-strong bg-panel p-4">
            <div>
              <p className="mb-2 text-mono-code uppercase text-ink-500">Indexing metadata</p>
              <dl className="space-y-1 text-mono-code text-ink-400">
                <Row label="Slug" value={library.slug} />
                <Row label="Versions" value={`${library.versions.length} indexed`} />
                <Row label="Last indexed" value={library.last_indexed_at ? relativeTimeFromIsoString(library.last_indexed_at) : "never"} />
                <Row label="Changelog" value={library.changelog_versions > 0 ? "Available" : "None"} />
                <Row label="Used in" value={`${library.used_in_count} repo${library.used_in_count === 1 ? "" : "s"}`} />
              </dl>
            </div>
            <div className="space-y-1.5 border-t border-border-strong pt-3">
              <p className="mb-2 text-mono-code uppercase text-ink-500">Actions</p>
              <Button variant="outline" size="sm" className="w-full justify-start" asChild>
                <Link href={`/search?library=${encodeURIComponent(library.slug)}`}>
                  <Search className="size-3.5" />
                  Search this library
                </Link>
              </Button>
              <RescrapeButton slug={library.slug} version={viewingVersion} />
            </div>
          </div>
        </div>
      </Tabs>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between gap-2">
      <dt className="text-ink-500">{label}:</dt>
      <dd className="truncate text-ink-300">{value}</dd>
    </div>
  );
}

function LibraryDocsPane({ slug, version }: { slug: string; version: string }) {
  const { data: text, isLoading } = useSWR(["library-docs", slug, version], () => getLibraryDocs(slug, version));
  if (isLoading) return <p className="text-body text-ink-500">Loading docs…</p>;
  if (!text) return <p className="text-body text-ink-500">No docs found for this version.</p>;
  return <MarkdownContent text={text} />;
}

function LibraryChangelogPane({ slug }: { slug: string }) {
  const { data: text, isLoading } = useSWR(["library-changelog", slug], () => getLibraryChangelog(slug));

  if (isLoading) return <p className="text-body text-ink-500">Loading changelog…</p>;
  if (!text || text.startsWith("No changelog entries synced")) {
    return <p className="text-body text-ink-500">No changelog synced for this library yet — run sync_changelogs from its detail page actions.</p>;
  }
  return <MarkdownContent text={text} />;
}

function RescrapeButton({ slug, version }: { slug: string; version: string }) {
  const [pending, setPending] = useState(false);

  async function handleClick() {
    setPending(true);
    try {
      const result = await rescrapeLibrary(slug, version);
      if (result.isError) toast.error(result.text);
      else toast.success(result.text);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Re-index failed. Please try again.");
    } finally {
      setPending(false);
    }
  }

  return (
    <Button variant="outline" size="sm" className="w-full justify-start" disabled={pending} onClick={handleClick}>
      <RefreshCw className={pending ? "size-3.5 animate-spin" : "size-3.5"} />
      Re-index
    </Button>
  );
}
