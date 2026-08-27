"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { ChevronDown, Search, Share2 } from "lucide-react";
import type { DocPage } from "@/lib/api/repos-api";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { RepoPicker } from "@/components/graph/repo-picker";

export function DocsHeader({
  repo,
  page,
  onChangeRepo,
  onViewInGraph,
}: {
  repo: string;
  page: DocPage | undefined;
  onChangeRepo: (repo: string) => void;
  onViewInGraph: () => void;
}) {
  const router = useRouter();
  const [query, setQuery] = useState("");

  return (
    <div className="flex h-[52px] shrink-0 items-center justify-between border-b border-border-strong px-5">
      <div className="flex items-center gap-2.5 text-section">
        <span className="text-ink-400">Documentation</span>
        <span className="text-ink-500">/</span>
        <Popover>
          <PopoverTrigger asChild>
            <button
              type="button"
              className="flex items-center gap-1.5 rounded border border-border-strong px-2 py-1 text-mono-code transition-colors hover:border-border-strong/80"
            >
              <span className="text-ink-200">{repo}</span>
              <ChevronDown className="size-3 text-ink-500" />
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-64">
            <RepoPicker onSelect={onChangeRepo} />
          </PopoverContent>
        </Popover>
        {page && <span className="text-mono-code text-ink-500">Indexed {page.generated_at}</span>}
      </div>
      <div className="flex items-center gap-2">
        <div className="relative">
          <Search className="absolute left-2.5 top-1/2 size-3 -translate-y-1/2 text-ink-500" />
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && query.trim()) router.push(`/search?q=${encodeURIComponent(query.trim())}`);
            }}
            placeholder="Search docs…"
            className="w-44 rounded-md border border-border-strong bg-canvas py-1.5 pl-7 pr-3 text-mono-code text-ink-100 placeholder-ink-500 outline-none focus:border-primary/50"
          />
        </div>
        <button
          type="button"
          onClick={onViewInGraph}
          title="View in graph"
          className="flex size-7 items-center justify-center rounded border border-border-strong text-ink-400 transition-colors hover:border-border-strong/80 hover:text-ink-100"
        >
          <Share2 className="size-3.5" />
        </button>
      </div>
    </div>
  );
}
