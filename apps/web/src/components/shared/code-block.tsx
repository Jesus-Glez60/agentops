import { Highlight, themes } from "prism-react-renderer";
import { CopyButton } from "@/components/shared/copy-button";

// vsDark is the closest bundled theme to this app's own dark palette --
// applied via Prism's per-token inline styles, so the block's own
// background/border still come from our design tokens, not the theme.
const CODE_THEME = themes.vsDark;

export function CodeBlock({ code, title, language }: { code: string; title?: string; language?: string }) {
  return (
    <div className="overflow-hidden rounded-md border border-border-strong bg-raised">
      {title && (
        <div className="flex items-center justify-between border-b border-border-strong px-3 py-1.5">
          <span className="text-mono-path text-ink-500">{title}</span>
          <CopyButton value={code} label="Copy" />
        </div>
      )}
      <Highlight theme={CODE_THEME} code={code} language={language ?? "tsx"}>
        {({ className, style, tokens, getLineProps, getTokenProps }) => (
          <pre className={`${className} overflow-x-auto p-3 text-mono-code`} style={{ ...style, backgroundColor: "transparent" }}>
            {tokens.map((line, i) => (
              <div key={i} {...getLineProps({ line })}>
                {line.map((token, key) => (
                  <span key={key} {...getTokenProps({ token })} />
                ))}
              </div>
            ))}
          </pre>
        )}
      </Highlight>
    </div>
  );
}
