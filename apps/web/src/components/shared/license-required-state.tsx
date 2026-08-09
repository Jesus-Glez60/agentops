import Link from "next/link";
import { Lock } from "lucide-react";
import { Button } from "@/components/ui/button";

/**
 * The dedicated state for a 402 from agentops-heavy-api's /search* routes
 * -- distinct from a generic ErrorState, since "not licensed" isn't a bug,
 * it's an expected, paid-tier-gated response.
 */
export function LicenseRequiredState({ feature = "Semantic search" }: { feature?: string }) {
  return (
    <div className="flex flex-col items-center gap-3 rounded-md border border-dashed border-border-strong py-16 text-center">
      <Lock className="size-6 text-ink-500" />
      <div>
        <p className="text-section font-medium text-ink-100">{feature} is a paid-tier feature</p>
        <p className="mt-1 max-w-sm text-body text-ink-500">
          This deployment has no valid license or Qdrant configuration for semantic search. Set
          <code className="mx-1 rounded bg-raised px-1 py-0.5 text-mono-code">AGENTOPS_LICENSE_KEY</code>
          and <code className="mx-1 rounded bg-raised px-1 py-0.5 text-mono-code">AGENTOPS_QDRANT_URL</code>
          on agentops-heavy-api to enable it.
        </p>
      </div>
      <Button asChild variant="outline" size="sm">
        <Link href="/">Back to Overview</Link>
      </Button>
    </div>
  );
}
