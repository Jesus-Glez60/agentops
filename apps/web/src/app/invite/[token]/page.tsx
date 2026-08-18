import { getCurrentUser } from "@/lib/auth/session";
import { InviteLandingClient } from "@/components/team/invite-landing-client";

export default async function InvitePage({ params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  // Soft check -- unlike (app)/layout.tsx's requireUser(), a visitor
  // previewing an invite link may not be signed in yet at all; this page
  // renders either way and branches on whether `user` is null.
  const user = await getCurrentUser();

  return <InviteLandingClient token={token} isSignedIn={user !== null} />;
}
