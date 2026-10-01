"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import useSWR from "swr";
import { toast } from "sonner";
import { getInvitePreview, acceptInvite } from "@/lib/api/team-api";
import { Button } from "@/components/ui/button";
import { AuthSplitLayout, GradientText } from "@/components/auth/auth-split-layout";

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

  const orgName = preview?.org_name || "an organization";

  return (
    <AuthSplitLayout
      eyebrow="You're invited"
      eyebrowColor="var(--teal-400)"
      headline={
        <>
          Join <GradientText stops={["#94e2d5", "#89b4fa", "#cba6f7"]}>{orgName}</GradientText> on AgentOps
        </>
      }
      subtitle={isLoading ? "Loading invite…" : error ? "This invite link is invalid or has expired." : `You've been invited as ${ROLE_LABELS[preview!.role] ?? preview!.role}.`}
      leftExtra={
        preview ? (
          <div className="flex items-center gap-3 rounded-xl border border-border-strong bg-panel p-4">
            <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-raised text-[18px] font-bold text-ink-100">{orgName.charAt(0).toUpperCase()}</span>
            <div className="flex flex-col">
              <span className="font-medium text-ink-100">{orgName}</span>
              <span className="text-body text-ink-500">Joining as {ROLE_LABELS[preview.role] ?? preview.role}</span>
            </div>
          </div>
        ) : undefined
      }
    >
      <div className="flex flex-col gap-5 rounded-2xl border border-border-strong bg-panel p-6">
        <h2 className="text-display-card font-bold text-ink-100">Team invite</h2>
        {error && <p className="text-body text-ink-500">This invite link is invalid or has expired.</p>}
        {preview && (
          <>
            {isSignedIn ? (
              <>
                <p className="text-body text-ink-500">Accepting will make this your active organization — you can switch back to any other org you belong to afterward from the menu in the sidebar.</p>
                <Button className="w-full" disabled={accepting} onClick={handleAccept}>
                  {accepting ? "Joining…" : "Accept invite"}
                </Button>
              </>
            ) : (
              <Button className="w-full" asChild>
                <Link href={`/login?from=${encodeURIComponent(`/invite/${token}`)}`}>Log in or sign up to accept</Link>
              </Button>
            )}
          </>
        )}
      </div>
    </AuthSplitLayout>
  );
}
