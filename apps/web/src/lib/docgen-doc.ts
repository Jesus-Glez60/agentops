export interface DocSymbol {
  name: string;
  startLine?: number;
  endLine?: number;
  gotchaCount: number;
}

export interface DocFile {
  path: string;
  symbols: DocSymbol[];
}

export interface DocNote {
  title: string;
  body: string;
  affects: { name: string; path: string }[];
}

export interface ParsedDoc {
  title: string;
  subtitle: string;
  stats: { label: string; value: string }[];
  repoMapIntro: string;
  files: DocFile[];
  gotchas: DocNote[];
  decisions: DocNote[];
}

const H1 = /^#\s+(.+)$/;
const H3_FILE = /^###\s+`(.+)`$/;
const STAT_LINE = /^-\s+([^:]+):\s*(.+)$/;
const SYMBOL_LINE = /^-\s+`([^`]+)`\s+\(lines\s+(\d+)-(\d+)\)/;
const NOTE_TITLE = /^\*\*(.+)\*\*$/;
const AFFECTS_LINE = /^-\s+Affects:\s+`([^`]+)`\s+in\s+`([^`]*)`$/;

const STATS_HEADING = /^##\s+Stats\s*$/i;
const REPO_MAP_HEADING = /^##\s+Repository map\s*$/i;
const GOTCHAS_HEADING = /^##\s+Known gotchas\s*$/i;
const DECISIONS_HEADING = /^##\s+Decisions\s*$/i;

/**
 * The ingested gotcha/decision notes are real user-authored markdown (see
 * apps/web session notes: 61 vault notes bulk-ingested via `agentops note`)
 * and routinely contain their own `##`/`###` headings in the note body
 * itself (e.g. a gotcha's body starting with `## Context`). A naive "any
 * line matching `^##\s` is a new top-level section" scanner mis-fires on
 * those, corrupting the whole parse (wrong doc title picked up from deep in
 * a note body, phantom file entries). Real docgen section headings are only
 * ever followed by one of these fixed sentences (see agentops-docgen's
 * `render_onboarding_doc`) -- requiring that exact follow-up line as
 * confirmation is what actually distinguishes a structural heading from a
 * coincidental one inside free-form note content.
 */
function nextNonBlank(lines: string[], from: number): string | undefined {
  for (let j = from; j < lines.length; j++) {
    if (lines[j].trim() !== "") return lines[j].trim();
  }
  return undefined;
}

function isRealGotchasHeading(lines: string[], idx: number): boolean {
  if (!GOTCHAS_HEADING.test(lines[idx])) return false;
  return (nextNonBlank(lines, idx + 1) ?? "").toLowerCase().includes("every recorded workaround/issue");
}

function isRealDecisionsHeading(lines: string[], idx: number): boolean {
  if (!DECISIONS_HEADING.test(lines[idx])) return false;
  return (nextNonBlank(lines, idx + 1) ?? "").toLowerCase().includes("every recorded design decision");
}

/** Parses a run of `**title**` / body / `- Affects:` note blocks starting at `lines[start]`, stopping at `stopAt(lines, idx)` or EOF. Returns the index just past the last consumed line. */
function parseNotes(lines: string[], start: number, out: DocNote[], stopAt?: (lines: string[], idx: number) => boolean): number {
  let i = start;
  let current: DocNote | null = null;

  function flush() {
    if (current) {
      current.body = current.body.trim();
      out.push(current);
    }
    current = null;
  }

  while (i < lines.length) {
    if (stopAt?.(lines, i)) break;

    const line = lines[i];
    const titleMatch = NOTE_TITLE.exec(line.trim());
    // Real note titles (agentops-notes' `add_gotcha`/`add_decision` -- the
    // CLI's free-form `<TITLE>` argument) are descriptive phrases and never
    // end in a colon. A note's own body prose very often contains
    // standalone bold "run-in heading" lines as an informal sub-structure
    // (`**Cause:**`, `**Fix (required pattern):**`) that otherwise match the
    // exact same `**...**`-alone-on-a-line shape as a real title. Excluding
    // colon-terminated matches is what actually separates the two in real
    // ingested content -- confirmed against all 61 vault notes ingested for
    // the CurrentYachts demo, where every false split had this shape.
    if (titleMatch && !titleMatch[1].trim().endsWith(":")) {
      flush();
      current = { title: titleMatch[1], body: "", affects: [] };
      i++;
      continue;
    }
    if (!current) {
      i++;
      continue;
    }
    const affects = AFFECTS_LINE.exec(line.trim());
    if (affects) {
      current.affects.push({ name: affects[1], path: affects[2] });
      i++;
      continue;
    }
    if (line.trim() || current.body) {
      current.body += (current.body ? "\n" : "") + line;
    }
    i++;
  }
  flush();
  return i;
}

/**
 * Parses agentops-docgen's real, deterministic markdown output (see
 * agentops-core's agentops-docgen crate: `render_onboarding_doc`/
 * `render_notes`) back into structured data, so the Documentation page can
 * render each section with its own purpose-built UI (a searchable file/
 * symbol browser, styled gotcha/decision cards) instead of dumping ~800+
 * files as one flat, unbroken markdown wall. This intentionally mirrors the
 * exact heading/bullet shape that Rust code produces -- it is not a general
 * markdown parser, and callers should fall back to rendering the raw
 * markdown if the expected top-level headings aren't found (a sign this is
 * no longer agentops-docgen's output, or the format changed).
 */
export function parseDocgenMarkdown(markdown: string): ParsedDoc | null {
  const lines = markdown.split("\n");
  let i = 0;

  let title = "";
  for (; i < lines.length; i++) {
    const h1 = H1.exec(lines[i]);
    if (h1) {
      title = h1[1].trim();
      i++;
      break;
    }
  }
  if (!title) return null;

  let subtitle = "";
  while (i < lines.length && lines[i].trim() === "") i++;
  if (i < lines.length && lines[i].trim().startsWith("_")) {
    subtitle = lines[i].trim().replace(/^_+|_+$/g, "");
    i++;
  }

  while (i < lines.length && !STATS_HEADING.test(lines[i])) i++;
  if (i >= lines.length) return null;
  i++;

  const stats: { label: string; value: string }[] = [];
  while (i < lines.length && !/^##\s+/.test(lines[i])) {
    const m = STAT_LINE.exec(lines[i].trim());
    if (m) stats.push({ label: m[1].trim(), value: m[2].trim() });
    i++;
  }

  if (i >= lines.length || !REPO_MAP_HEADING.test(lines[i])) return null;
  i++;

  let repoMapIntro = "";
  while (i < lines.length && lines[i].trim() === "") i++;
  if (i < lines.length && !H3_FILE.test(lines[i]) && !/^##\s+/.test(lines[i])) {
    repoMapIntro = lines[i].trim();
    i++;
  }

  const files: DocFile[] = [];
  while (i < lines.length && !isRealGotchasHeading(lines, i) && !isRealDecisionsHeading(lines, i)) {
    const h3 = H3_FILE.exec(lines[i]);
    if (!h3) {
      i++;
      continue;
    }
    const file: DocFile = { path: h3[1], symbols: [] };
    i++;
    while (i < lines.length && !H3_FILE.test(lines[i]) && !isRealGotchasHeading(lines, i) && !isRealDecisionsHeading(lines, i)) {
      const trimmed = lines[i].trim();
      const sym = SYMBOL_LINE.exec(trimmed);
      if (sym) {
        const gotchaMatch = /\*\*(\d+) known gotcha/.exec(trimmed);
        file.symbols.push({
          name: sym[1],
          startLine: Number(sym[2]),
          endLine: Number(sym[3]),
          gotchaCount: gotchaMatch ? Number(gotchaMatch[1]) : 0,
        });
      }
      i++;
    }
    files.push(file);
  }

  const gotchas: DocNote[] = [];
  const decisions: DocNote[] = [];

  if (i < lines.length && isRealGotchasHeading(lines, i)) {
    i++;
    i = parseNotes(lines, i, gotchas, (l, idx) => isRealDecisionsHeading(l, idx));
  }
  if (i < lines.length && isRealDecisionsHeading(lines, i)) {
    i++;
    i = parseNotes(lines, i, decisions);
  }

  return { title, subtitle, stats, repoMapIntro, files, gotchas, decisions };
}
