import { requireUser } from "@/lib/auth/session";
import { publicApiUrl } from "@/lib/server/public-api-url";
import { WelcomeWizard } from "@/components/onboarding/welcome-wizard";

// Top-level, deliberately outside (app)/ -- that layout redirects here
// whenever `!user.onboarding_completed`, so living inside it would loop.
// Session-authed like every (app)/* route (requireUser redirects to
// /login if there's no session), but rendered full-bleed like /login and
// /setup rather than inside the app shell/sidebar.
export default async function WelcomePage() {
  const user = await requireUser();
  const { url: apiUrl, isGuess: apiUrlIsGuessed } = await publicApiUrl();
  return <WelcomeWizard user={user} apiUrl={apiUrl} apiUrlIsGuessed={apiUrlIsGuessed} />;
}
