"use client";

// PM2 deployment's infra-config wizard (Method 2 in the deployment plan) --
// collects the same fields a Docker `.env` or `agentops init` would, POSTs
// them to POST /bootstrap/config (agentops-heavy-api's `bootstrap_config`
// handler), which writes `.env` and 403s once any account already exists
// (see that handler's doc comment). Deliberately does NOT create an
// account itself -- org/user setup stays the browser `/login` -> signup
// flow, which is what actually assigns Owner (see `agentops-teams::
// ensure_membership`, called lazily on first authenticated request, not at
// signup). This page's only job is infra config.
import { useState, type FormEvent } from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";

function randomHex(bytes: number): string {
  const arr = new Uint8Array(bytes);
  crypto.getRandomValues(arr);
  return Array.from(arr, (b) => b.toString(16).padStart(2, "0")).join("");
}

export default function SetupPage() {
  const router = useRouter();
  const [masterKey, setMasterKey] = useState(() => randomHex(32));
  const [addr, setAddr] = useState("0.0.0.0:8420");
  const [usePostgres, setUsePostgres] = useState(false);
  const [databaseUrl, setDatabaseUrl] = useState("");
  const [openSignup, setOpenSignup] = useState(false);
  const [pending, setPending] = useState(false);
  const [errors, setErrors] = useState<string[]>([]);
  const [done, setDone] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setErrors([]);
    setPending(true);
    try {
      const res = await fetch("/api/bootstrap/config", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          secrets_master_key: masterKey,
          addr,
          database_url: usePostgres && databaseUrl ? databaseUrl : undefined,
          signup_mode: openSignup ? "open" : "first-user-only",
        }),
      });
      const data = await res.json().catch(() => null);
      if (!res.ok) {
        const problems: string[] = Array.isArray(data?.errors) ? data.errors : [typeof data?.error === "string" ? data.error : `request failed with ${res.status}`];
        setErrors(problems);
        toast.error(problems.length === 1 ? problems[0] : "Please fix the highlighted problems.");
        return;
      }
      setDone(true);
    } catch {
      toast.error("Unable to reach the backend.");
    } finally {
      setPending(false);
    }
  }

  if (done) {
    return (
      <main className="flex min-h-screen items-center justify-center bg-canvas p-4">
        <Card className="w-full max-w-md">
          <CardHeader>
            <CardTitle className="text-page-title">Config saved</CardTitle>
            <CardDescription>Wrote .env — restart the app for it to take effect (e.g. pm2 restart ecosystem.config.js), then continue to sign in.</CardDescription>
          </CardHeader>
          <CardFooter>
            <Button className="w-full" onClick={() => router.push("/login")}>
              Continue to sign in
            </Button>
          </CardFooter>
        </Card>
      </main>
    );
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-canvas p-4">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle className="text-page-title">Set up AgentOps</CardTitle>
          <CardDescription>One-time infra configuration for this deployment. Org and user setup comes next, on the sign-in page.</CardDescription>
        </CardHeader>
        <form onSubmit={handleSubmit}>
          <CardContent className="space-y-4">
            <div className="space-y-1.5">
              <label htmlFor="setup-master-key" className="text-section text-ink-300">
                Secrets master key
              </label>
              <div className="flex gap-2">
                <Input id="setup-master-key" value={masterKey} onChange={(e) => setMasterKey(e.target.value)} disabled={pending} className="font-mono text-xs" />
                <Button type="button" variant="outline" onClick={() => setMasterKey(randomHex(32))} disabled={pending}>
                  Regenerate
                </Button>
              </div>
            </div>

            <div className="space-y-1.5">
              <label htmlFor="setup-addr" className="text-section text-ink-300">
                Bind address
              </label>
              <Input id="setup-addr" value={addr} onChange={(e) => setAddr(e.target.value)} disabled={pending} />
            </div>

            <div className="space-y-1.5">
              <div className="flex items-center gap-2">
                <Checkbox id="setup-use-postgres" checked={usePostgres} onCheckedChange={(v) => setUsePostgres(v === true)} disabled={pending} />
                <label htmlFor="setup-use-postgres" className="text-section text-ink-300">
                  Use Postgres for the code-graph store
                </label>
              </div>
              {usePostgres && (
                <Input
                  aria-label="Postgres connection string"
                  placeholder="postgres://user:pass@host/db"
                  value={databaseUrl}
                  onChange={(e) => setDatabaseUrl(e.target.value)}
                  disabled={pending}
                />
              )}
            </div>

            <div className="flex items-center gap-2">
              <Checkbox id="setup-open-signup" checked={openSignup} onCheckedChange={(v) => setOpenSignup(v === true)} disabled={pending} />
              <label htmlFor="setup-open-signup" className="text-section text-ink-300">
                Allow open signup after the first account (not recommended for self-host)
              </label>
            </div>

            {errors.length > 1 && (
              <ul className="list-disc space-y-1 pl-5 text-body text-destructive">
                {errors.map((e) => (
                  <li key={e}>{e}</li>
                ))}
              </ul>
            )}
          </CardContent>
          <CardFooter>
            <Button type="submit" className="w-full" disabled={pending}>
              {pending ? "Saving…" : "Save configuration"}
            </Button>
          </CardFooter>
        </form>
      </Card>
    </main>
  );
}
