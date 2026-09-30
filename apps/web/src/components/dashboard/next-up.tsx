import Link from "next/link";
import { parseRepoStatus, type RepoConnection } from "@/lib/api/repos-api";

interface NextUpCard {
  tag: string;
  color: string;
  title: string;
  desc: string;
  cta: string;
  href: string;
}

// Three real signals, prioritized in this order -- matches the prototype's
// own card ordering. Never padded to 3: a card only appears when its
// backing data actually says so (see redesign plan's Session 3 update,
// Section B).
export function buildNextUpCards(repos: RepoConnection[], needsCurationCount: number): NextUpCard[] {
  const cards: NextUpCard[] = [];

  if (needsCurationCount > 0) {
    cards.push({
      tag: "Curate",
      color: "var(--peach)",
      title: `${needsCurationCount} gotcha${needsCurationCount === 1 ? "" : "s"} need${needsCurationCount === 1 ? "s" : ""} a decision`,
      desc: "Agents recorded these during sessions. Keep, pin or reduce them so the right ones reach agents first.",
      cta: "Start review →",
      href: "/gotchas",
    });
  }

  const discovered = repos.find((r) => r.method === "discovered");
  if (discovered) {
    cards.push({
      tag: "Finish connecting",
      color: "var(--blue-500)",
      title: `An agent found ${discovered.id}`,
      desc: "It's registered but has no credentials yet, so nothing has been indexed. Add a deploy key to scan it.",
      cta: "Finish connecting →",
      href: `/repositories/connect/ssh?repo_url=${encodeURIComponent(discovered.repo_url)}`,
    });
  }

  // Deliberately distinct from the "Curate" signal above: a real scan
  // failure or a vanished checkout path, not the generic gotchas-need-
  // curation warning (that's already what the Curate card covers) --
  // showing both for the same underlying repo would be a duplicate signal.
  const needsAttention = repos.find((r) => r.path_missing || parseRepoStatus(r.status).kind === "failed");
  if (needsAttention) {
    const parsed = parseRepoStatus(needsAttention.status);
    cards.push({
      tag: "Needs attention",
      color: "var(--yellow)",
      title: `${needsAttention.id} scanned with warnings`,
      desc: needsAttention.path_missing ? "Repo path no longer exists." : parsed.kind === "failed" ? parsed.reason : "Check this repository's scan status.",
      cta: "View repository →",
      href: `/repositories/${encodeURIComponent(needsAttention.id)}`,
    });
  }

  return cards.slice(0, 3);
}

export function NextUp({ cards }: { cards: NextUpCard[] }) {
  if (cards.length === 0) return null;

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-subheading font-semibold text-ink-100">Next up</h2>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {cards.map((card) => (
          <div key={card.tag} className="flex flex-col gap-2 rounded-lg border border-border-strong bg-panel p-4">
            <span className="text-mono-path uppercase tracking-wide" style={{ color: card.color }}>
              {card.tag}
            </span>
            <h3 className="text-section font-semibold text-ink-100">{card.title}</h3>
            <p className="flex-1 text-body text-ink-500">{card.desc}</p>
            <Link href={card.href} className="text-section font-medium" style={{ color: card.color }}>
              {card.cta}
            </Link>
          </div>
        ))}
      </div>
    </section>
  );
}
