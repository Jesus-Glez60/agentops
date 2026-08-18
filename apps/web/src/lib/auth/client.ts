// Client-side auth calls -- always through this app's own /api/auth/* BFF
// routes, never straight to agentops-heavy-api (see heavy-api.ts's doc
// comment for why). The login/signup forms call these two functions
// instead of inlining fetch, so a future "or continue with…" method can be
// added here without restructuring the forms themselves.
import type { SessionUser, TwoFactorChallenge } from "@/lib/auth/types";

export class AuthClientError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "AuthClientError";
  }
}

async function postAuth(path: string, body: Record<string, string>): Promise<Record<string, unknown>> {
  const res = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await res.json().catch(() => null);

  if (!res.ok) {
    const message = data && typeof data === "object" && typeof data.error === "string" ? data.error : "Something went wrong. Please try again.";
    throw new AuthClientError(message);
  }

  return data as Record<string, unknown>;
}

/** Returns a `TwoFactorChallenge` instead of the signed-in user when the account has 2FA enabled -- callers must check `"two_factor_required" in result` before treating it as `SessionUser`. */
export async function loginWithPassword(email: string, password: string): Promise<SessionUser | TwoFactorChallenge> {
  const data = await postAuth("/api/auth/login", { email, password });
  return "two_factor_required" in data ? (data as unknown as TwoFactorChallenge) : (data.user as SessionUser);
}

export async function completeLogin2fa(challengeToken: string, code: string): Promise<SessionUser> {
  const data = await postAuth("/api/auth/login/2fa", { challenge_token: challengeToken, code });
  return data.user as SessionUser;
}

export async function signupWithPassword(firstName: string, lastName: string, email: string, password: string): Promise<SessionUser> {
  const data = await postAuth("/api/auth/signup", { first_name: firstName, last_name: lastName, email, password });
  return data.user as SessionUser;
}

export async function logout(): Promise<void> {
  await fetch("/api/auth/logout", { method: "POST" });
}
