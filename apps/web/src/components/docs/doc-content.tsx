import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { slugify } from "@/lib/markdown-toc";

// Minimal shape covering what we actually read off a hast node here --
// react-markdown's own types are more elaborate than this call site needs.
interface HastLikeNode {
  value?: string;
  children?: HastLikeNode[];
}

function nodeText(node: HastLikeNode | undefined): string {
  if (!node) return "";
  if (typeof node.value === "string") return node.value;
  return (node.children ?? []).map(nodeText).join("");
}

/**
 * Renders agentops-docgen's real markdown output (see agentops-core's
 * agentops-docgen crate -- `# {repo} overview`, `## Stats`, `## Repository
 * map` with per-file `### ` sub-headings, `## Known gotchas`, `##
 * Decisions`). Headings get the same slug id scheme markdown-toc.ts uses
 * for the TOC, in document order, so "on this page" links actually land on
 * the right heading.
 */
export function DocContent({ markdown }: { markdown: string }) {
  const slugCounts = new Map<string, number>();

  function slugFor(text: string): string {
    const base = slugify(text);
    const count = slugCounts.get(base) ?? 0;
    slugCounts.set(base, count + 1);
    return count > 0 ? `${base}-${count}` : base;
  }

  function heading(level: 1 | 2 | 3) {
    const Tag = `h${level}` as const;
    const sizeClass = level === 1 ? "text-page-title" : level === 2 ? "text-subheading" : "text-section";
    return function Heading({ node, children }: { node?: HastLikeNode; children?: React.ReactNode }) {
      const id = slugFor(nodeText(node));
      return (
        <Tag id={id} className={`${sizeClass} mt-6 mb-2 font-bold text-ink-100 first:mt-0`}>
          {children}
        </Tag>
      );
    };
  }

  const components: Components = {
    h1: heading(1),
    h2: heading(2),
    h3: heading(3),
    p: ({ children }) => <p className="mb-3 text-body text-ink-300">{children}</p>,
    ul: ({ children }) => <ul className="mb-3 ml-5 list-disc space-y-1 text-body text-ink-300">{children}</ul>,
    code: ({ children }) => <code className="rounded bg-raised px-1 py-0.5 text-mono-code text-ink-100">{children}</code>,
    pre: ({ children }) => <pre className="mb-3 overflow-x-auto rounded-md border border-border-strong bg-raised p-3 text-mono-code">{children}</pre>,
    a: ({ href, children }) => (
      <a href={href} className="text-primary underline underline-offset-2">
        {children}
      </a>
    ),
  };

  return (
    <div className="max-w-3xl">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {markdown}
      </ReactMarkdown>
    </div>
  );
}
