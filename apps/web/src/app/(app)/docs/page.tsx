"use client";

import { Suspense, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import useSWR from "swr";
import { getDocs, DOCS_SWR_KEY } from "@/lib/api/agentops-api";
import { DocsHeader } from "@/components/docs/docs-header";
import { DocsNav } from "@/components/docs/docs-nav";
import { DocsContent } from "@/components/docs/docs-content";
import { RepoPicker } from "@/components/graph/repo-picker";

export default function DocsPage() {
  // useSearchParams requires a Suspense boundary during static generation.
  return (
    <Suspense fallback={null}>
      <DocsPageInner />
    </Suspense>
  );
}

function DocsPageInner() {
  const searchParams = useSearchParams();
  const router = useRouter();
  const [repoName, setRepoName] = useState<string | null>(searchParams.get("repo"));
  // Each nav item is its own "page" (a query param, not a scroll anchor) --
  // a repo with a lot of generated content (this one, easily 1000+ symbols)
  // never turns into one unbounded-height document. `null` until the repo's
  // sections load, resolved to the first real section right below.
  const [sectionId, setSectionId] = useState<string | null>(searchParams.get("section"));
  // Which callout is open within an all-callout section (Notes/Known
  // Gotchas/Architectural Decisions) -- `null` means that section's list
  // view. Lifted here (not local to the content pane) so the left nav's
  // book-index sub-list can open/advance the same item.
  const [itemIndex, setItemIndex] = useState<number | null>(null);

  const { data: page } = useSWR(repoName ? [DOCS_SWR_KEY, repoName] : null, () => getDocs(repoName!));
  const activeSection = page ? (page.sections.find((s) => s.id === sectionId) ?? page.sections[0]) : undefined;

  function changeRepo(repo: string) {
    setRepoName(repo);
    setSectionId(null);
    setItemIndex(null);
  }

  function selectSection(id: string) {
    setSectionId(id);
    setItemIndex(null);
  }

  function selectItem(id: string, index: number) {
    setSectionId(id);
    setItemIndex(index);
  }

  function viewInGraph(nodeId?: number) {
    if (!repoName) return;
    router.push(nodeId ? `/graph?repo=${encodeURIComponent(repoName)}&node=${nodeId}` : `/graph?repo=${encodeURIComponent(repoName)}`);
  }

  if (!repoName) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex h-[52px] shrink-0 items-center border-b border-border-strong px-5 text-section font-medium text-ink-100">Documentation</div>
        <div className="mx-auto flex w-full max-w-md flex-1 flex-col justify-center gap-3">
          <div className="flex flex-col items-center gap-1 text-center">
            <p className="text-subheading text-ink-100">Pick a repository</p>
            <p className="text-body text-ink-500">Generated engineering documentation for a scanned repo, ordered by graph centrality.</p>
          </div>
          <RepoPicker onSelect={changeRepo} />
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <DocsHeader repo={repoName} page={page} onChangeRepo={changeRepo} onViewInGraph={() => viewInGraph()} />
      <div className="flex min-h-0 flex-1 overflow-hidden">
        {page && (
          <DocsNav
            sections={page.sections}
            activeSectionId={activeSection?.id ?? ""}
            activeItemIndex={itemIndex}
            onSelectSection={selectSection}
            onSelectItem={selectItem}
          />
        )}
        <div className="min-h-0 min-w-0 flex-1 overflow-y-auto">
          {page && activeSection ? (
            <DocsContent page={page} section={activeSection} selectedItem={itemIndex} onSelectItem={setItemIndex} onViewInGraph={viewInGraph} />
          ) : (
            <p className="p-8 text-body text-ink-500">Loading documentation…</p>
          )}
        </div>
      </div>
    </div>
  );
}
