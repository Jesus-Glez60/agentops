import Link from "next/link";

export default function ReposPage() {
  return (
    <main className="mx-auto max-w-3xl px-8 py-16">
      <h1 className="text-2xl font-semibold">Repo overview</h1>
      <p className="mt-2 text-zinc-600 dark:text-zinc-400">
        Phase 1 stub — will list connected repos, last-scanned time, and health status
        from agentops-api once agentops-scanner/agentops-graph are implemented.
      </p>
      <p className="mt-4 text-sm text-zinc-600 dark:text-zinc-400">
        Looking to connect a repo for hosted (heavy-tier) access instead?{" "}
        <Link href="/repos/connect" className="underline">
          GitHub App / SSH deploy-key connection flow →
        </Link>
      </p>
    </main>
  );
}
