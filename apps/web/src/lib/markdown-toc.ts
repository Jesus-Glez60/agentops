export interface TocEntry {
  level: number;
  text: string;
  slug: string;
}

/** GitHub-style slugification -- lowercase, spaces to hyphens, strip anything
 * that isn't alphanumeric/hyphen/underscore. Good enough for headings this
 * app itself generates (agentops-docgen output), not general markdown. */
export function slugify(text: string): string {
  return text
    .toLowerCase()
    .trim()
    .replace(/[^\w\s-]/g, "")
    .replace(/\s+/g, "-");
}

/**
 * Extracts a flat table of contents from `#`/`##`/`###` headings in raw
 * markdown -- the real structure agentops-docgen's output actually has
 * (Stats / Repository map / per-file sub-headings / Known gotchas /
 * Decisions), not a fabricated "Core Modules / Execution Flows" taxonomy a
 * mockup might imply but the real generator doesn't produce.
 */
export function extractToc(markdown: string): TocEntry[] {
  const entries: TocEntry[] = [];
  const seenSlugs = new Map<string, number>();

  for (const line of markdown.split("\n")) {
    const match = /^(#{1,3})\s+(.+)$/.exec(line);
    if (!match) continue;
    const level = match[1].length;
    const text = match[2].trim();
    let slug = slugify(text);
    const count = seenSlugs.get(slug) ?? 0;
    if (count > 0) slug = `${slug}-${count}`;
    seenSlugs.set(slugify(text), count + 1);
    entries.push({ level, text, slug });
  }

  return entries;
}
