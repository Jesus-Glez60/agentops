"use client";

import { useEffect, useState } from "react";

type Connection = {
  id: string;
  tenant: string;
  repo_url: string;
  method: string;
  public_key_openssh: string | null;
  status: string;
  created_at: string;
};

const API_BASE = process.env.NEXT_PUBLIC_HEAVY_API_URL || "http://127.0.0.1:8978";

export default function ConnectRepoPage() {
  const [tenant, setTenant] = useState("");
  const [repoId, setRepoId] = useState("");
  const [repoUrl, setRepoUrl] = useState("");
  const [connections, setConnections] = useState<Connection[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [verifyingId, setVerifyingId] = useState<string | null>(null);
  const [installUrl, setInstallUrl] = useState<string | null>(null);
  const [installUrlChecked, setInstallUrlChecked] = useState(false);

  useEffect(() => {
    fetch(new URL("/repos/github-app/install-url", API_BASE))
      .then((res) => (res.ok ? res.json() : null))
      .then((data) => setInstallUrl(data?.install_url ?? null))
      .catch(() => setInstallUrl(null))
      .finally(() => setInstallUrlChecked(true));
  }, []);

  const loadConnections = (t: string) => {
    if (!t.trim()) {
      setConnections(null);
      return;
    }
    const url = new URL("/repos", API_BASE);
    url.searchParams.set("tenant", t.trim());
    fetch(url.toString())
      .then((res) => {
        if (!res.ok) throw new Error(`agentops-heavy-api returned ${res.status}`);
        return res.json();
      })
      .then((data) => setConnections(data.connections))
      .catch((e) => setError(`Could not reach agentops-heavy-api at ${API_BASE} (${e instanceof Error ? e.message : String(e)}).`));
  };

  const handleConnect = (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setConnecting(true);
    fetch(new URL("/repos/connect", API_BASE), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ tenant: tenant.trim(), repo_id: repoId.trim(), repo_url: repoUrl.trim() }),
    })
      .then(async (res) => {
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `agentops-heavy-api returned ${res.status}`);
        return data;
      })
      .then(() => {
        setRepoId("");
        setRepoUrl("");
        loadConnections(tenant);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setConnecting(false));
  };

  const handleVerify = (id: string) => {
    setVerifyingId(id);
    const url = new URL(`/repos/${encodeURIComponent(id)}/verify`, API_BASE);
    url.searchParams.set("tenant", tenant.trim());
    fetch(url.toString(), { method: "POST" })
      .then((res) => res.json())
      .then(() => loadConnections(tenant))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setVerifyingId(null));
  };

  return (
    <main className="mx-auto max-w-3xl px-8 py-16">
      <h1 className="text-2xl font-semibold">Connect a repo (heavy tier)</h1>
      <p className="mt-2 text-zinc-600 dark:text-zinc-400">
        Hosted repo access for the commercial heavy tier — talks to <code className="text-sm">agentops-heavy-api</code>, not the
        light-tier <code className="text-sm">agentops-api</code>. Two paths: a GitHub App install (recommended — GitHub holds the
        credential, revocable from GitHub&apos;s own UI) or a per-repo SSH deploy key (self-hosted fallback, generated below).
      </p>

      <section className="mt-8 rounded-md border border-black/[.08] p-4 dark:border-white/[.145]">
        <h2 className="text-sm font-medium text-zinc-500">GitHub App</h2>
        {!installUrlChecked && <p className="mt-2 text-sm text-zinc-500">Checking…</p>}
        {installUrlChecked && installUrl && (
          <a
            href={installUrl}
            target="_blank"
            rel="noreferrer"
            className="mt-2 inline-block rounded-md bg-foreground px-4 py-2 text-sm text-background hover:bg-[#383838] dark:hover:bg-[#ccc]"
          >
            Install GitHub App
          </a>
        )}
        {installUrlChecked && !installUrl && (
          <p className="mt-2 text-sm text-zinc-500">
            No GitHub App is configured for this deployment yet — use the SSH deploy-key flow below instead.
          </p>
        )}
      </section>

      <form onSubmit={handleConnect} className="mt-8 space-y-3 rounded-md border border-black/[.08] p-4 dark:border-white/[.145]">
        <h2 className="text-sm font-medium text-zinc-500">SSH deploy key</h2>
        <input
          type="text"
          value={tenant}
          onChange={(e) => {
            setTenant(e.target.value);
            loadConnections(e.target.value);
          }}
          placeholder="tenant / org id"
          required
          className="w-full rounded-md border border-black/[.08] bg-transparent px-3 py-2 text-sm dark:border-white/[.145]"
        />
        <input
          type="text"
          value={repoId}
          onChange={(e) => setRepoId(e.target.value)}
          placeholder="repo id (e.g. widgets)"
          required
          className="w-full rounded-md border border-black/[.08] bg-transparent px-3 py-2 text-sm dark:border-white/[.145]"
        />
        <input
          type="text"
          value={repoUrl}
          onChange={(e) => setRepoUrl(e.target.value)}
          placeholder="git@github.com:org/repo.git"
          required
          className="w-full rounded-md border border-black/[.08] bg-transparent px-3 py-2 text-sm font-mono dark:border-white/[.145]"
        />
        <button
          type="submit"
          disabled={connecting}
          className="rounded-md bg-foreground px-4 py-2 text-sm text-background transition-colors hover:bg-[#383838] disabled:opacity-50 dark:hover:bg-[#ccc]"
        >
          {connecting ? "Generating…" : "Generate deploy key"}
        </button>
      </form>

      {error && <div className="mt-4 rounded-md border border-red-500/30 bg-red-500/5 p-4 text-sm text-red-700 dark:text-red-400">{error}</div>}

      <div className="mt-8">
        {connections === null && tenant.trim() === "" && <p className="text-sm text-zinc-500">Enter a tenant id to see its connections.</p>}
        {connections !== null && connections.length === 0 && <p className="text-sm text-zinc-500">No connections for this tenant yet.</p>}
        {connections !== null && connections.length > 0 && (
          <div className="space-y-4">
            {connections.map((c) => (
              <div key={c.id} className="rounded-md border border-black/[.08] p-4 dark:border-white/[.145]">
                <div className="flex items-center justify-between">
                  <span className="font-mono text-sm">{c.repo_url}</span>
                  <span
                    className={
                      "rounded px-2 py-0.5 text-xs " +
                      (c.status === "active"
                        ? "bg-green-500/10 text-green-700 dark:text-green-400"
                        : c.status.startsWith("failed")
                          ? "bg-red-500/10 text-red-700 dark:text-red-400"
                          : "bg-zinc-500/10 text-zinc-600 dark:text-zinc-400")
                    }
                  >
                    {c.status}
                  </span>
                </div>
                {c.public_key_openssh && (
                  <div className="mt-3">
                    <p className="text-xs text-zinc-500">
                      Paste this as a read-only Deploy Key on the repo (Settings → Deploy Keys), then verify:
                    </p>
                    <code className="mt-1 block overflow-x-auto rounded bg-black/[.03] p-2 text-xs dark:bg-white/[.05]">
                      {c.public_key_openssh}
                    </code>
                  </div>
                )}
                <button
                  onClick={() => handleVerify(c.id)}
                  disabled={verifyingId === c.id}
                  className="mt-3 rounded-md border border-black/[.08] px-3 py-1.5 text-xs hover:bg-black/[.03] disabled:opacity-50 dark:border-white/[.145] dark:hover:bg-white/[.05]"
                >
                  {verifyingId === c.id ? "Verifying…" : "Verify"}
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </main>
  );
}
