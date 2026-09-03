"use client";

// The browser-side half of the `gh auth login`-style CLI device-
// authorization flow: a CLI (possibly on a different, headless machine)
// printed this page's URL with a `?code=` pre-filled from
// `verification_uri_complete`; this card shows that code and lets the
// already-logged-in user (guaranteed by `/cli-auth/page.tsx`'s
// `requireUser()`) approve or deny it.
import { useState } from "react";
import { toast } from "sonner";
import { approveDeviceAuth, denyDeviceAuth } from "@/lib/api/profile-api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";
import { Input } from "@/components/ui/input";

export function DeviceApprovalCard({ initialCode, userName }: { initialCode?: string; userName: string }) {
  const [code, setCode] = useState(initialCode ?? "");
  const [submitting, setSubmitting] = useState<"approve" | "deny" | null>(null);
  const [resolution, setResolution] = useState<"approved" | "denied" | null>(null);

  async function resolve(action: "approve" | "deny") {
    if (!code.trim()) return;
    setSubmitting(action);
    try {
      if (action === "approve") {
        await approveDeviceAuth(code.trim());
      } else {
        await denyDeviceAuth(code.trim());
      }
      setResolution(action === "approve" ? "approved" : "denied");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't process that code. Please try again.");
    } finally {
      setSubmitting(null);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-canvas p-4">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle className="text-page-title">Connect a coding tool</CardTitle>
          <CardDescription>
            {resolution ? null : `Hi ${userName} — a command-line tool wants to connect to your AgentOps account.`}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {resolution === "approved" ? (
            <p className="text-body text-ink-300">You can close this tab and return to your terminal.</p>
          ) : resolution === "denied" ? (
            <p className="text-body text-ink-300">Request denied. You can close this tab.</p>
          ) : (
            <>
              <Input value={code} onChange={(e) => setCode(e.target.value.toUpperCase())} placeholder="XXXX-XXXX" className="text-center text-mono-code text-lg tracking-widest" />
              <div className="flex gap-2">
                <Button className="flex-1" disabled={!code.trim() || submitting !== null} onClick={() => resolve("approve")}>
                  {submitting === "approve" ? "Approving…" : "Approve"}
                </Button>
                <Button className="flex-1" variant="outline" disabled={!code.trim() || submitting !== null} onClick={() => resolve("deny")}>
                  {submitting === "deny" ? "Denying…" : "Deny"}
                </Button>
              </div>
            </>
          )}
        </CardContent>
      </Card>
    </main>
  );
}
