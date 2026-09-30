"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import useSWR from "swr";
import { toast } from "sonner";
import { Mail } from "lucide-react";
import { getInvitePreview, acceptInvite } from "@/lib/api/team-api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { LogoMark } from "@/components/shared/logo-mark";

const ROLE_LABELS: Record<string, string> = { admin: "Admin", member: "Member", viewer: "Viewer", billing: "Billing" };

export function InviteLandingClient({ token, isSignedIn }: { token: string; isSignedIn: boolean }) {
  const router = useRouter();
  const { data: preview, error, isLoading } = useSWR(["invite-preview", token], () => getInvitePreview(token));
  const [accepting, setAccepting] = useState(false);

  async function handleAccept() {
    setAccepting(true);
    try {
      await acceptInvite(token);
      toast.success("You've joined the team — this is now your active organization. Switch back any time from the org menu in the sidebar.");
      router.push("/");
      router.refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't accept this invite. Please try again.");
    } finally {
      setAccepting(false);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-canvas p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <div className="mb-2 flex size-10 items-center justify-center rounded-lg border border-border-strong bg-panel">
            <Mail className="size-4 text-ink-400" />
          </div>
          <CardTitle className="flex items-center gap-2 text-page-title">
            <LogoMark className="size-5 shrink-0" />
            Team invite
          </CardTitle>
          {isLoading && <CardDescription>Loading…</CardDescription>}
          {error && <CardDescription>This invite link is invalid or has expired.</CardDescription>}
          {preview && (
            <CardDescription>
              You&apos;ve been invited to join <span className="font-medium text-ink-100">{preview.org_name || "an organization"}</span> as{" "}
              <span className="font-medium text-ink-100">{ROLE_LABELS[preview.role] ?? preview.role}</span>.
            </CardDescription>
          )}
        </CardHeader>
        {preview && (
          <CardContent>
            {isSignedIn ? (
              <>
                <p className="mb-3 text-body text-ink-500">Accepting will make this your active organization — you can switch back to any other org you belong to afterward from the menu in the sidebar.</p>
                <Button className="w-full" disabled={accepting} onClick={handleAccept}>
                  {accepting ? "Joining…" : "Accept invite"}
                </Button>
              </>
            ) : (
              <Button className="w-full" asChild>
                <Link href={`/login?from=${encodeURIComponent(`/invite/${token}`)}`}>Log in or sign up to accept</Link>
              </Button>
            )}
          </CardContent>
        )}
      </Card>
    </main>
  );
}
