import { Fragment, type ReactNode } from "react";

// Gotcha/Decision/Note content is hand-written Markdown (see .agentops/notes'
// own convention) but rendered as plain text, so **bold** and `code` spans
// showed up as literal asterisks/backticks. This isn't a general Markdown
// renderer -- just the two inline spans that content actually uses -- so no
// new Markdown-parser dependency for two token types.
function renderInline(text: string, keyPrefix: string): ReactNode[] {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).filter(Boolean);
  return parts.map((part, i) => {
    if (part.startsWith("**") && part.endsWith("**")) {
      return (
        <strong key={`${keyPrefix}-${i}`} className="font-semibold text-ink-100">
          {part.slice(2, -2)}
        </strong>
      );
    }
    if (part.startsWith("`") && part.endsWith("`")) {
      return (
        <code key={`${keyPrefix}-${i}`} className="rounded bg-canvas px-1 py-0.5 text-mono-code text-ink-100">
          {part.slice(1, -1)}
        </code>
      );
    }
    return <Fragment key={`${keyPrefix}-${i}`}>{part}</Fragment>;
  });
}

export function Prose({ text, className }: { text: string; className?: string }) {
  const paragraphs = text.split(/\n{2,}/);
  return (
    <div className={className}>
      {paragraphs.map((paragraph, i) => (
        <p key={i} className={i > 0 ? "mt-3" : undefined}>
          {paragraph.split("\n").map((line, j, lines) => (
            <Fragment key={j}>
              {renderInline(line, `${i}-${j}`)}
              {j < lines.length - 1 && <br />}
            </Fragment>
          ))}
        </p>
      ))}
    </div>
  );
}
