/**
 * A short, plain-text preview of a markdown note body -- for compact cards
 * (symbol-attached knowledge, the flat gotchas/decisions list) where
 * rendering or even showing the full body is what caused those sections to
 * "go on forever." Strips the most common markdown noise (headings, list
 * markers, emphasis, inline code) rather than rendering to HTML, since this
 * is a plain-text summary, not a formatted preview.
 */
export function excerptFromMarkdown(content: string | null | undefined, maxLength = 160): string {
  if (!content) return "";
  const plain = content
    .split("\n")
    .map((line) => line.replace(/^#{1,6}\s+/, "").replace(/^[-*]\s+/, ""))
    .join(" ")
    .replace(/`{1,3}[^`]*`{1,3}/g, (m) => m.replace(/`/g, ""))
    .replace(/[*_>#]/g, "")
    .replace(/\s+/g, " ")
    .trim();
  if (plain.length <= maxLength) return plain;
  return plain.slice(0, maxLength).trimEnd() + "…";
}
