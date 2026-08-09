"use client";

import { NodeBadge } from "@/components/shared/node-badge";
import { HealthBadge, type HealthStatus } from "@/components/shared/health-badge";
import { RelevanceBadge, type RelevanceLevel } from "@/components/shared/relevance-badge";
import { RelationshipChip } from "@/components/shared/relationship-chip";
import { StatCard } from "@/components/shared/stat-card";
import { KnowledgeCallout } from "@/components/shared/knowledge-callout";
import { CodeBlock } from "@/components/shared/code-block";
import { EmptyState } from "@/components/shared/empty-state";
import { ErrorState } from "@/components/shared/error-state";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Database, Search } from "lucide-react";
import type { NodeKind } from "@/lib/api/types";

const TOKEN_GROUPS: { name: string; className: string }[] = [
  { name: "canvas", className: "bg-canvas" },
  { name: "panel", className: "bg-panel" },
  { name: "raised", className: "bg-raised" },
  { name: "border-strong", className: "bg-border-strong" },
  { name: "ink-100", className: "bg-ink-100" },
  { name: "ink-300", className: "bg-ink-300" },
  { name: "ink-500", className: "bg-ink-500" },
  { name: "slate-blue (structural)", className: "bg-slate-blue" },
];

const NODE_KINDS: NodeKind[] = ["Symbol", "File", "Gotcha", "Decision"];
const HEALTH_STATUSES: HealthStatus[] = ["healthy", "scanning", "warning", "stale", "failed", "not-indexed"];
const RELEVANCE_LEVELS: RelevanceLevel[] = ["strong", "related", "supporting"];

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-3 border-b border-border-strong pb-8">
      <h2 className="text-label uppercase tracking-wide text-ink-500">{title}</h2>
      {children}
    </section>
  );
}

/**
 * Every swatch below renders the exact same components used elsewhere in
 * the app with fixture props -- not a hand-copied mockup -- so this page
 * structurally cannot drift from the real UI. The only way it goes stale
 * is someone adding a new variant without adding a swatch here, which is
 * an ordinary living-styleguide discipline problem, not an architecture one.
 */
export default function DesignSystemPage() {
  return (
    <div className="flex max-w-3xl flex-col gap-8">
      <h1 className="text-page-title font-bold">Design System</h1>

      <Section title="Color palette">
        <div className="grid grid-cols-4 gap-3">
          {TOKEN_GROUPS.map((t) => (
            <div key={t.name} className="flex flex-col gap-1">
              <div className={`h-12 rounded-md border border-border-strong ${t.className}`} />
              <span className="text-mono-code text-ink-500">{t.name}</span>
            </div>
          ))}
        </div>
      </Section>

      <Section title="Typography">
        <div className="flex flex-col gap-2">
          <p className="text-page-title font-bold text-ink-100">Repository Overview (page-title, 20/700)</p>
          <p className="text-subheading font-semibold text-ink-100">Authentication (subheading, 16/600)</p>
          <p className="text-section font-medium text-ink-100">Section heading (section, 13/500)</p>
          <p className="text-body text-ink-300">Body text — explanatory content and descriptions (body, 13/400)</p>
          <p className="text-mono-path text-ink-500">src/auth/session.ts:42-68 (mono-path, 11px)</p>
          <p className="text-mono-code text-ink-100">refreshSession() (mono-code, 12px)</p>
          <p className="text-label uppercase tracking-wide text-ink-500">Section label (label, 10px)</p>
        </div>
      </Section>

      <Section title="Node-type badges">
        <div className="flex flex-wrap gap-2">
          {NODE_KINDS.map((k) => (
            <NodeBadge key={k} kind={k} />
          ))}
        </div>
      </Section>

      <Section title="Health indicators">
        <div className="flex flex-wrap gap-4">
          {HEALTH_STATUSES.map((s) => (
            <HealthBadge key={s} status={s} />
          ))}
        </div>
      </Section>

      <Section title="Relevance indicators">
        <div className="flex flex-wrap gap-2">
          {RELEVANCE_LEVELS.map((l) => (
            <RelevanceBadge key={l} level={l} />
          ))}
        </div>
      </Section>

      <Section title="Relationship chips">
        <div className="flex flex-wrap gap-2">
          <RelationshipChip relation="affects" target="refreshSession()" />
          <RelationshipChip relation="depends on" target="permissions.ts" />
          <RelationshipChip relation="← Documents" target="onboarding doc" onClick={() => {}} />
        </div>
      </Section>

      <Section title="Stat cards">
        <div className="grid grid-cols-3 gap-3">
          <StatCard label="Repositories" value={4} icon={Database} />
          <StatCard label="Gotchas requiring review" value={3} icon={Search} valueClassName="text-health-warning" />
          <StatCard label="Stale index" value={1} icon={Database} valueClassName="text-health-stale" />
        </div>
      </Section>

      <Section title="Inline knowledge callouts">
        <div className="flex flex-col gap-2">
          <KnowledgeCallout kind="Gotcha" relation="affects" target="refreshSession()">
            Both access and refresh tokens must be persisted after session refresh.
          </KnowledgeCallout>
          <KnowledgeCallout kind="Decision" relation="applies to" target="auth module">
            Deploy keys are restricted to read-only access per repository.
          </KnowledgeCallout>
        </div>
      </Section>

      <Section title="Code components">
        <CodeBlock
          title="src/auth/session.ts"
          code={`export async function refreshSession(ctx: SessionCtx) {\n  const { accessToken, refreshToken } =\n    await rotateTokenPair(ctx.refreshToken);\n}`}
        />
      </Section>

      <Section title="Buttons & inputs">
        <div className="flex flex-wrap items-center gap-2">
          <Button>Primary</Button>
          <Button variant="outline">Secondary</Button>
          <Button variant="destructive">Destructive</Button>
          <Button disabled>Disabled</Button>
        </div>
        <Input placeholder="Search..." className="max-w-xs" />
      </Section>

      <Section title="Empty, loading & error states">
        <div className="grid grid-cols-3 gap-3">
          <EmptyState icon={Search} title="No results found" description="Try adjusting your query or filter." />
          <div className="flex flex-col gap-2 rounded-md border border-border-strong p-4">
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-1/2" />
          </div>
          <ErrorState message="Indexing failed at: Repository clone" />
        </div>
      </Section>
    </div>
  );
}
