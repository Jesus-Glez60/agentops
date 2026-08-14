import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

// `react-markdown`/`remark-gfm` were already dependencies but never actually
// wired up anywhere (the hand-rolled `Prose` component only handles two
// inline spans, not real Markdown) -- this is the first real usage, for
// docbrain's scraped doc/changelog content, which is genuine multi-heading
// Markdown, not just bold/code spans. No @tailwindcss/typography plugin is
// installed, so headings/lists/etc. are hand-styled here with this app's
// existing text-ink-*/text-mono-code tokens rather than a `prose` class.
const components: Components = {
  h1: ({ children }) => <h1 className="mb-3 mt-6 text-lg font-semibold text-ink-100 first:mt-0">{children}</h1>,
  h2: ({ children }) => <h2 className="mb-2 mt-5 text-base font-semibold text-ink-100 first:mt-0">{children}</h2>,
  h3: ({ children }) => <h3 className="mb-2 mt-4 text-body font-semibold text-ink-100">{children}</h3>,
  h4: ({ children }) => <h4 className="mb-1.5 mt-3 text-body font-semibold text-ink-200">{children}</h4>,
  p: ({ children }) => <p className="mb-3 text-body leading-relaxed text-ink-300">{children}</p>,
  a: ({ children, href }) => (
    <a href={href} target="_blank" rel="noreferrer" className="text-primary underline-offset-2 hover:underline">
      {children}
    </a>
  ),
  strong: ({ children }) => <strong className="font-semibold text-ink-100">{children}</strong>,
  ul: ({ children }) => <ul className="mb-3 list-disc space-y-1 pl-5 text-body text-ink-300">{children}</ul>,
  ol: ({ children }) => <ol className="mb-3 list-decimal space-y-1 pl-5 text-body text-ink-300">{children}</ol>,
  li: ({ children }) => <li>{children}</li>,
  hr: () => <hr className="my-4 border-border-strong" />,
  blockquote: ({ children }) => <blockquote className="mb-3 border-l-2 border-border-strong pl-3 text-ink-400 italic">{children}</blockquote>,
  code: ({ children, className }) => {
    // A fenced block's `code` has a `language-*` className from remark;
    // an inline span doesn't -- same signal `react-markdown`'s own docs
    // recommend for telling the two apart without a separate `pre`-only path.
    const isBlock = /language-/.test(className ?? "");
    if (isBlock) {
      return <code className={className}>{children}</code>;
    }
    return <code className="rounded bg-canvas px-1 py-0.5 text-mono-code text-ink-100">{children}</code>;
  },
  pre: ({ children }) => <pre className="mb-3 overflow-x-auto rounded-md border border-border-strong bg-canvas p-3 text-mono-code text-ink-200">{children}</pre>,
  table: ({ children }) => (
    <div className="mb-3 overflow-x-auto">
      <table className="w-full border-collapse text-body">{children}</table>
    </div>
  ),
  th: ({ children }) => <th className="border-b border-border-strong px-2 py-1.5 text-left font-medium text-ink-200">{children}</th>,
  td: ({ children }) => <td className="border-b border-border-strong px-2 py-1.5 text-ink-300">{children}</td>,
};

export function MarkdownContent({ text, className }: { text: string; className?: string }) {
  return (
    <div className={className}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
