import { CopyButton } from "@/components/shared/copy-button";

export function CodeBlock({ code, title, language }: { code: string; title?: string; language?: string }) {
  return (
    <div className="overflow-hidden rounded-md border border-border-strong bg-raised">
      {title && (
        <div className="flex items-center justify-between border-b border-border-strong px-3 py-1.5">
          <span className="text-mono-path text-ink-500">{title}</span>
          <CopyButton value={code} label="Copy" />
        </div>
      )}
      <pre className="overflow-x-auto p-3 text-mono-code text-ink-100">
        <code className={language ? `language-${language}` : undefined}>{code}</code>
      </pre>
    </div>
  );
}
